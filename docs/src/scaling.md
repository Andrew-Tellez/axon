# Scaling and load

All of this is numbers, and **a declared number nobody checks is an opinion**. axon
checks them with arithmetic:

```toml
[infra]
state           = "postgres"
ha              = true      # a standby with failover. Nobody reads from it
backup_retention_days = 30  # HA is not a backup: the standby replicates a DROP in seconds
pitr            = true
pool_size       = 8         # connections per instance
max_connections = 200       # the engine's ceiling
read_replicas   = 0         # these ARE read from, and they lag
shard_key       = "tenant_id"
```

## The distinction almost everybody confuses

An **HA standby** and a **read replica** are not the same thing, and the compiler
enforces it: nobody reads from the standby, it exists so the service stays up, and that
is why it **breaks no consistency**. A read replica IS read from, it lags, and that is
why it **does** break it — declaring `read_replicas > 0` with `consistency = "strong"`
is an error.

And a third thing that is neither of those two: **HA is not a backup.** A standby
replicates a `DROP TABLE` in seconds. That is why a `tier = "0"` requires both, with at
least 7 days of retention: a logical delete is discovered after the weekend, not in the
next minute.

## The arithmetic nobody does

```console
$ axon verify manifests/
error: orders: 10 connections x 10 instances = 100, plus 2 reserved, goes past the
       ceiling of 100. The service falls over from exhaustion when it scales, not when
       you test it: lower the pool, lower max_instances, or put a pooler in front
```

That error showed up over this repo's own examples the first time I wrote realistic
numbers. **Connection exhaustion does not show up with one instance: it shows up the day
it scales**, and it is a multiplication nobody does.

## Sharding: the rules nobody else enforces

With a `shard_key`, `verify` blocks five things that **raise no error, only wrong data**.
And nobody else checks this: PgDog's schema validator is on its roadmap unstarted, and
Citus only fails at runtime when distributing the table.

| | |
| --- | --- |
| A table with no shard key | it cannot be sharded |
| A `UNIQUE` that does not include the key | **each node honours it separately and the set does not**: two nodes accept the same value with no error |
| A `serial` / `IDENTITY` column | each node has its own sequence, starting at 1: the values collide |
| `tenant_column` ≠ `shard_key` | isolating by one column and sharding by another makes **every** query of one tenant touch **every** node |
| `pitr = true` + `shard_key` | N nodes are N timelines: there is no consistent recovery point for the set |
| An FK between a sharded table and one that is not | it crosses nodes |

Two exceptions, because **a rule with false positives gets silenced**: a composite
`UNIQUE` that *does* include the key is safe —each node guarantees it— and a `uuid`
column is unique by construction worldwide, so a uuid PK is not flagged.

## The pooler and the sharding

```toml
[pooler]
engine          = "pgdog"
mode            = "transaction"
tenant_binding  = "set_local"   # mandatory in `transaction` with tenants
shards          = 4
max_client_conn = 500
pool_size       = 40
```

```sh
axon pooler manifests/ > pgdog.toml
```

Out comes [pgdog](https://pgdog.dev)'s configuration with the nodes, and **the sharding
derived from the real schema**: which tables carry the key and what type it is, read from
the migrations. The connection details come out as variables — a generated file is no
place for a password.

Two decisions the generator makes on its own, and why:

- **`cross_shard_disabled = true`.** With no cross-node `JOIN` and no global uniqueness,
  a query that crosses is answered *wrong*. Rejecting it turns each of the sharder's
  limitations into a loud error.
- **`query_parser = "on"`, not `"auto"`.** In `auto` the parser does not kick in with a
  single primary node — which is exactly the case where a session variable slips through
  uncaught. And the symptom **disappears when a replica is added**, so it does not
  reproduce in a staging that has one.

### The manifest rule that discovers itself

With the database sharded by tenant, **a method that does not receive the tenant cannot
be served**. The router rejects the query and the sharder does not know which node to go
to. It came out of running the demo: `getOrder` looked up by its primary key and pgdog
rejected it on the very first request.

```console
$ axon verify manifests/
error  orders.getOrder: does not receive `tenant_id` and the database is sharded by
       that column. The router rejects a query that does not filter by tenant (`no
       multi tenant id`), and the sharder does not know which node to send it to. Add
       it to `in`, and usually to the route as well
```

Looking up by primary key stops being enough, and that is not a limitation of the
pooler: it is what sharding by tenant means.

### The rule that matters

```console
$ axon verify manifests/
error  p: `mode = "transaction"` with `tenant_column` and no `tenant_binding =
       "set_local"`. The connection goes back to the pool at every COMMIT and is handed
       to another tenant: a session GUC survives and the next request reads the previous
       tenant's rows, with no error. `SET LOCAL` dies with the transaction
```

In transaction mode the physical connection is recycled between tenants. If the tenant is
pinned with a session `SET`, the value survives and **the next request reads the previous
one's rows**. That does not fail: it returns the wrong customer's data.

The other pooler rules change the **subject** of the arithmetic, which is the part that
gets overlooked:

| | |
| --- | --- |
| `pool_size` × `max_instances` > `max_client_conn` | with a pooler in the middle, the instances compete for *its* clients, not for the engine's connections |
| `pooler.pool_size` + reserved > `max_connections` | **per engine**: the nodes and the replicas are different engines, each with its own ceiling |
| `mode = "session"` with more clients than connections | in session mode there is no multiplexing: one client connection ties up one server connection |
| `shards > 1` with `consistency = "strong"` | two-phase commit makes partial states visible: the real guarantee is eventual |
| `shards > 1` with no `shard_key` | the sharder needs the column, and `verify` needs to check that every table carries it |
| `[pooler]` fields with `engine = "none"` | configuration that applies nowhere |

### Verified against its own schema

pgdog publishes the **official JSON Schema** of its configuration, generated from its
Rust types and checked by its CI. The suite validates the generated `pgdog.toml` against
that file: that is validating against the real parser that will read it, not against our
idea of how it should look.

### On k8s the nodes are not axon's; pgdog is

On `--target k8s` the databases are managed instances the team owns, and they arrive as a
secret. pgdog is the part axon does own: it is the piece that has to be configured
exactly right, and configuring it by hand is where a shard ends up pointing at the wrong
host. So the target renders the two generated files in a `ConfigMap`, a pgdog
`Deployment` pinned by digest, its `Service` on 6432, and a `NetworkPolicy` where only
its own service gets in.

The generated file is a **template**: `port = ${AXON_DB_PORT_0}` is not even valid TOML.
The substitution travels with it as an initContainer, generated from the same text as the
markers, so neither can leave the other behind — and an unset variable stops the pod
**naming** what is missing instead of leaving a literal `${…}` that pgdog rejects with a
parse error nobody traces back to a secret.

The service's `DATABASE_URL` points at `pooler-<service>:6432`, never at a node: pointing
at a node skips the sharding and everything works — against a quarter of the data.

The suite runs that substitution with a real `sh` and validates what comes out against
pgdog's official schema. Before this, the only thing that filled those markers was the
test's own regex.

### And on a managed cloud, a sidecar

On `gcp` and `aws` the nodes are N managed instances —Cloud SQL or RDS, one **instance**
per node, because a shard sharing an engine with the other three shares its ceiling, its
CPU and its outage— and pgdog cannot be a service of its own: Cloud Run and ECS serve
HTTP, and the Postgres wire protocol needs a process the app reaches on localhost. So it
goes as a **sidecar**, mounted from a secret whose value axon does not know.

That changes the arithmetic, and the change is the interesting part: the pool stops being
one and becomes **one per instance**. axon does the multiplication before the apply and
refuses when it does not fit:

```console
$ axon infra manifests/ --target gcp
axon: orders: on `gcp` the sharder is a sidecar —Cloud Run and ECS serve HTTP, and the
Postgres protocol needs a process next to the app—, so the pool is one PER INSTANCE:
40 x 10 instances = 400, plus 2 reserved, over the limit of 100 per node. Lower
`[pooler] pool_size`, lower `max_instances`, or raise the node's `max_connections`
```

That is a better refusal than the blanket one it replaces: it names the number, where it
came from and the three ways out. Connection exhaustion does not show up when you test
with one instance — it shows up the day it scales.

The app's `DATABASE_URL` comes from a **different secret** (`<service>-pooler-url`) than
the unsharded one, so nobody points the service at a node by reusing the old URL: that
skips the sharding and works, against a quarter of the data.

Both renders go through `terraform validate` with the real providers in the suite: the
sidecar is a second container, a volume from a secret and a `dependsOn`, and an attribute
that does not exist there would only show up on the apply.

### And measured through the pooler

The `local` target brings pgdog up in front of the N Postgres nodes, and the demo
measures what was in doubt: that the generated RLS **keeps isolating through the pooler
in transaction mode**.

```console
==> tenant isolation through the pooler
  a query with no tenant in the WHERE
  OK: pgdog rejects it at the router, before touching a node
  20 connections alternating tenant, each also asking for the other one's
  OK: 20 of 20 saw 1 row of their own and 0 of the tenant they asked for
```

Two layers that do not protect against the same thing: `[multi_tenant]` lives in the
query's text, so the pooler cannot weaken it; the RLS lives in the connection's GUC,
which is exactly what the pooler recycles. The first one rejects the query that names no
tenant; the second, the one that names the wrong one.

## The load test walks into the limit

A flat test at the declared rate answers *does it hold what we said*. That is worth
knowing and it is not the interesting question: it never sees what happens **one step
past** the limit, which is the moment that decides whether the thing degrades or falls
over.

`axon load` emits a ramp out of `rate_limit`: half of it, the declared rate, a stretch
holding there, and 25% over.

```js
stages: [
  { target: 30, duration: `${step}s` },   // half of it
  { target: 60, duration: `${step}s` },   // the declared rate_limit
  { target: 60, duration: `${step}s` },   // holding there
  { target: 75, duration: `${step}s` },   // 25% over: does it degrade or fall over
]
```

And the distinction the ramp exists to make: **a 429 is the declared limit working; a
5xx is the service breaking.** k6 counts both as `http_req_failed`, so the script keeps
two metrics of its own and the verdict reads them:

```console
$ axon load manifests/orders.toml --check summary.json
info: orders: 96 requests measured at 2.4/s
axon: 0 thresholds breached
    throttled 51%  ·  5xx 0%  ·  respuestas esperadas 100%
```

Not one 429 in a whole ramp is reported too: either the declared limit is never reached,
or **nobody is enforcing it** — and then it is a number in a document.

Which is what it used to be. `rate_limit` travelled to `k8s` as an annotation for
somebody else's controller and did nothing at all on `local`: the load test walked right
past it. Now the local edge carries the middleware, with the limit per minute —the unit
it is declared in— and a burst of a tenth, because traffic that is not perfectly smooth
would otherwise get throttled below its own declared limit.

## The engine has to exist

`state` is validated against a closed list. It used to be a free string, so
`state = "neo4j"` passed `verify` with no error and generated a Cloud SQL **Postgres**
instance: wrong output, silently.

```console
$ axon verify manifests/
error  g: `state = "neo4j"` is not supported. Native engines: postgres. A different
       one is served by an `axon-infra-neo4j` plugin, which receives the neutral plan
       on stdin
```

Today the only native engine is `postgres`. The plan is to support more families —time
series, graph, columnar, document— and the natural order is the **Postgres extensions**
(TimescaleDB, Apache AGE, pgvector), because they reuse the SQL parser, the migrations,
the RLS and the four targets that already exist. Until then, declaring another engine
fails and says how to proceed.

And `database per service` came to mean one **instance** per service: with all of them on
the same one, a noisy neighbour takes them down together, so it was not isolation.

```sh
axon load manifests/orders.toml > load.js
k6 run --summary-export=r.json load.js
axon load manifests/orders.toml --check r.json
```

The thresholds are **not numbers picked by eye**: each scenario runs at the rate its
`rate_limit` declares and fails if the p95 goes past its `timeout_ms`. It is the same
`diff` as `axon seq` against `axon trace`, applied to performance — and the script writes
down the ceiling the declared pool imposes, so you know whether the bottleneck is the
database or the code.

A route with a parameter is tested with a made-up id, because axon does not know your
data: there a 404 is **not** a service failure, and the script says so instead of
pretending it measures a successful read.

And `axon build` publishes the manifest's routes in `httpRoutes`, so startup can refuse
when a handler is missing. A route declared and not served returns a 404 in production
and shows up in no test — it happened in this repo's example, and the load test found it.
