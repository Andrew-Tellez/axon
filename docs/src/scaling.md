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
