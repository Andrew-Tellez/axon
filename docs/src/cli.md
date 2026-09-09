# Command reference

Every command writes to stdout and touches no disk. `axon verify` exits with code 1 if
there are errors; the rest exit 0 or fail with a message on stderr.

`<sources>` is a list of directories, `.toml` files or URLs of live services. A directory
takes its `*.toml` except the ones starting with `axon.` (those are the tool's own
configuration). A URL without `.json` resolves to `<url>/.well-known/axon.json`; a
service that is down is reported and does not break the rest.

# Code and contracts

## `axon build <manifest> [sources...] [--lang ts]`
Typed contracts, the envelope and the abstract base class.

The `sources` are the other manifests, and they are needed as soon as the service
consumes something: **a consumed event's type is declared by its emitter**, not by
whoever receives it. Without them, `build` fails saying what to pass instead of
generating code that does not compile. With `[patterns] outbox` the emitters write into
the outbox and `bus.publish` disappears from the file. If there is `consumes`, it
generates `dispatch()` with deduplication by id.

Besides the contract it emits: each `[machine]`'s transition table, the `isolationLevel`
that follows from the declared CAP side, the clients of each `[[depends]]` with their
policy running (timeout, backoff with jitter, breaker), and —if there are `pii` fields—
the list and a recursive `redact()`.

`--lang go` is native too, and it is the same manifest coming out as idiomatic Go: an
interface the person implements instead of inheritance, `ctx` first and `error` last,
`OrderID` and not `OrderId`. A `--lang` beyond those two looks for `axon-gen-<lang>` on
the `PATH` and passes it `{"manifest": ..., "peers": [...]}` on stdin.

## `axon test <manifest> <sources> [--lang ts] [--contracts ./contracts.ts]`
A testkit that **compiles on its own**: in-memory doubles of `Bus`, `Inbox` and `Outbox`,
fixtures derived from the schema of each event's **emitter**, and three exported suites.

It does not guess where your code lives: `contractTests` takes a factory. Weaving it in
is three hand-written lines:

```ts
import { contractTests, errorTests, machineTests } from "./axon.testkit.ts";
import { Payments } from "./index.ts";
contractTests((bus, inbox, outbox) => new Payments(bus, inbox, outbox, db));
machineTests();
errorTests();
```

`contractTests` checks what the manifest promises: that the handler accepts the event
exactly as its owner emits it, that a second delivery of the same id does not repeat the
effect, that the causal chain (`causationId`, `correlationId`, `traceparent`) survives the
handler, and that with `outbox` declared nothing is published straight to the bus.
`machineTests` does not need your code: it walks the transition table and verifies that
each transition is legal from its sources and illegal from any other state.

`FakeTransport` doubles the declared dependencies: it answers each one with a fixture of
**its** contract —cut down to what this service declared it `uses`— records what was
asked of it, and lets a test make one fail. That is how the declared policy gets
exercised with no network: a retriable failure has to arrive `1 + retries` times and a
final one exactly once.

```ts
const [clients, t] = fakeClients();
t.failWith("payments", "payoutMerchant", new AxonProblem(409, "merchant_ceiling"));
await assert.rejects(() => clients.paymentsPayoutMerchant(input, e));
assert.equal(t.timesCalled("payments", "payoutMerchant"), 1);   // final: it is not retried
```

`errorTests` needs none of it either, and it exists because the ends of a declared failure
are generated separately: `fail()` and the table on the callee's side, the `problem+json`
body on the wire, and the retriable list on the caller's client. It checks that they say
the same thing — the declared status and code on both sides, a trace on the body, an
undeclared error that does not come out looking declared, and nothing final offered as
retriable. See [`errors` on a method](./manifest.md#errors-on-a-method).

It runs with `node --test`, with no dependencies.

## `axon trace <file> [--seq] [--manifests <dir>]`
The real causal chain. It reads axon's envelope log, **OTLP JSON** or **Jaeger's** API
answer, and detects which — a span is an envelope with other names. With `--manifests`
it crosses the real edges between services against the declared ones, which is the half
`axon traffic` cannot see. See [Traceability](./traceability.md#and-for-a-system-that-never-adopted-the-envelope).

## `axon traffic <sources> --check <ndjson>`
Who calls what, read from the edge's access log. It answers the half of the question that
has an answer when the caller declares nothing: **what it asks for is observable, what it
reads of the answer is not**.

```console
$ axon traffic manifests/ --check .axon/log/edge.ndjson
19 requests  7 declared routes
        6    1  GET /v1/tenants/{tenantId}/orders/{orderId} · deprecated · 479d left · use `getOrderV2`
        5    1  POST /v1/tenants/{tenantId}/orders
        1    1  GET /v2/tenants/{tenantId}/orders/{orderId}
```

It reads NDJSON and is tolerant about the field names, because otherwise it would only
work with the edge axon itself generates. It reports routes with no traffic —saying that
a call between services does not pass through the edge, so it is not proof that nobody
calls them— and paths that match no declared route. It fails on one fact and not on a
judgement: traffic on something **past its declared sunset**, which is a retirement that
was announced and did not happen.

## `axon pact <sources> --check <pact.json>`
A pact from a consumer that never adopted axon. axon does not need a broker to **read**
one: their file already says which fields they need.

```console
$ axon pact manifests/ --check pacts/mobile-app-orders.json
mobile-app → orders  ·  2 interactions, 0 messages
  GET /v1/tenants/t-1/orders/o-1  →  orders.getOrder  ·  reads orderId, total.amount, total.currency
  mobile-app does not read status of getOrder
ok 2 interactions, 0 messages, 0 errors, 0 warnings
```

It answers the two things a foreign consumer leaves unanswered: whether it expects a
field nobody returns —renamed, or the pact is stale— and **which declared fields it does
not read**, which is the question that unfreezes a contract. It is not permission to
delete: another consumer may read it. It is one name off the list of unknowns.

A failure's body is compared as RFC 7807 and not against the method's output, and a
status the method does not declare comes out as a finding about the **provider**: it
fails that way and does not say so.

### And the topics

An endpoint is half the surface. A **message pact** —`messages[]` in v3, an interaction
with `type: Asynchronous/Messages` in v4— says which fields of an event a foreign
consumer reads, and that is something no amount of observation can answer: `axon traffic`
sees a call because it passes through the edge, but nobody sees who reads a message.

```console
$ axon pact manifests/ --check pacts/reporting-orders.json
reporting → orders  ·  0 interactions, 1 messages
  order.placed@v1  ·  reads orderId, total.amount, total.currency  (and traceparent, type of the envelope)
  reporting does not read customerId, customerEmail of order.placed@v1
ok 0 interactions, 1 messages, 0 errors, 0 warnings
```

The topic is read from `topic`, `kafka_topic`, `subject`, `destination` or `queue` in the
metadata —there is no single key, each implementation writes its own— and it matches both
spellings: the pact says `order.placed.v1`, the manifest says `order.placed@v1`. An event
that exists but belongs to **another** provider is reported as that and not as a missing
one: an event has exactly one owner, and the fix is a different one.

If the message carries the envelope, what the event promises is what is inside `data`;
reading something off the envelope that does not travel there is a finding of its own.

## `axon import <asyncapi|openapi> <file>`
An existing catalogue or an existing HTTP document turned into a manifest. See
[Getting in without rewriting anything](./importing.md).

## `axon accept <sources>`
The warnings the repo lives with for now. Emits the list to stdout; with
`axon.accepted.json` present, a warning that is not on it fails the build, and one that
stopped happening is reported so the list shrinks. See
[Getting in without rewriting anything](./importing.md#the-line-axon-accept).

## `axon tui <sources> [--frames N]`
The system as it is, drawn and animated: the topology as a force-directed graph —what
talks together ends up together— with the verdict, the versions, and what changed against
the baseline in the panels. `tab` cycles the panels, `space` pauses, `r` re-reads and `q`
quits.

An event and a call are drawn differently on purpose: the first travels on its own and
carries a pulse, the second is somebody waiting. A dependency on a version that is dying
is marked in the drawing, not only in `verify`.

With `--frames N` it renders N frames to stdout through ratatui's own test backend and
exits — which is what makes the picture checkable in CI, and usable in a pipe.

```console
$ axon tui manifests/ --frames 1
┌ axon ───────────────────────────────────────────── near · 0 errors 10 warn ┐
│                    orders [AP]                                             │
│                 ⢀⡠getOrder ⚠                                               │
│ payments [CP]⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀⣀ checkout [AP]                            │
│                     stripe [ext]                                           │
└────────────────────────────────────────────────────────────────────────────┘
┌ state ─────────────────────────────────────────────────────────────────────┐
│versions  path · getOrder deprecated · sunset 2027-12-31                    │
│changed   nothing new since the last baseline                               │
└───────────── [tab] panel  [space] pause  [r] re-read  [q] quit  ·  frame 4 ┘
```

## `axon analytics <sources> --metabase [--check <export.json>]`
Emits what a BI tool needs to read what the manifest declares: the connection to the
warehouse and one question per declared metric and per funnel that has a view. Emitted,
not applied — axon holds no credentials.

With `--check`, the other direction: what came back from the Metabase —its own
`/api/card` export— crossed against the schema axon generates. A question somebody wrote
**by hand** against an axon table names a column that no longer exists, and until now
nothing said so until somebody opened it. What it cannot read —a question that is not
native, one that joins or reads a CTE— is counted and named as not checked, never quietly
passed. See [Warehouse and business metrics](./analytics.md#the-drift-the-other-way).

## `axon rules <sources> [--check <tsv>]`
Without `--check` it emits the SQL that evaluates every declared `[rules.*]`: one row per
rule, series —the trigger and each guard— and window, with the value, the reference and
whether the condition holds there. It is emitted and not run, like `axon analytics
--check`: axon has no warehouse credentials and does not want them.

With `--check` it takes the decision over the rows that came back, and says what each
rule proposes or why it does not. It never applies anything.

## `axon versions <sources>`
The API's maintenance cycle, read out loud: which version is current, which are still
supported, how many days each has left, and what changed at each step. It does not block
— that is `verify`'s job — it tells you what nobody can see.

```console
$ axon versions manifests/
versioning = header (X-Api-Version)  3 versions · default 2026-09-01 · support window 365d

  2026-09-01  current  no death date

  2026-05-01  supported  267d left · sunset 2027-06-01
    · shop.getOrder  adapter downgradeGetOrderTo202605 · changed: customer

  2026-01-15  lts  861d left · sunset 2029-01-15
    · shop.getOrder  adapter downgradeGetOrderTo202601 · changed: status, customer
```

Under `versioning = "path"` it lists the routes that declared a retirement instead.

## `axon openapi <sources> [--api-version <date>]`
OpenAPI 3.1 for the whole platform in one document. `Idempotency-Key` mandatory on
mutating methods and `application/problem+json` (RFC 7807) as the uniform error.

With `--api-version` it emits the document **as of** a dated version: the shapes that
version promised, not today's. Under the header scheme every operation also carries the
version header as a parameter, with the declared versions as its `enum`.

## `axon discover <sources>`
A JSON registry: version, owner, methods with inputs and outputs, events emitted and
consumed. It works against disk and against running services, and merges both.

Each service serves its own manifest at `/.well-known/axon.json` —the path the generated
contract publishes— so the registry can be built from what is **deployed** instead of
from what somebody remembered to commit. A service running a manifest other than the
one in the repo compiles fine, because its contracts were generated from the old one;
comparing the two registries is the only place that shows, and the demo does it:

```console
==> the registry, from what is RUNNING
  OK: 3 services discovered live, and they declare what the repo says
```

A service that is down is reported and does not break the rest, and an external contract
is only on disk: it is frozen and serves nothing.

## `axon import asyncapi <file|-> [--service <name>]`
AsyncAPI 2.x or 3.x, JSON or YAML, to a manifest on stdout. The service's name comes from
`info.title` unless `--service` is passed.

In 2.x the direction is from outside the app: `publish` is what others publish towards it
(what the app **consumes**) and `subscribe` what it exposes for others to read (what it
**emits**). axon translates it; it is the number one confusion when reading 2.x.

What AsyncAPI does not declare comes out as `TODO` and `verify` treats it as absent.

## `axon crud <manifest> --expand`
Prints the methods a `[crud.*]` stands for, as TOML. It exists so overriding one is copy,
paste and edit: a method declared by hand **wins whole**, and this is what it would have
replaced. For an overridden endpoint it prints what axon *would* have generated and says
so — printing the override there would be a lie in the one place somebody reads to decide
whether to override.

There is no partial override on purpose. Two declarations of the same endpoint with merge
rules is a question nobody can answer at three in the morning.

## `axon init <service> [--path .]`
A project that verifies clean and comes up: the manifest, its migration, the Dockerfile the
compose builds, the policy and the `.env.local`, in the layout the rest of the CLI expects.

It exists because of what running the CLI on an empty directory found: the first three
failures were all **layout**. `migrations` resolves from the manifest, so `manifests/` next
to `sql/` reads nothing; the compose builds `services/<svc>/Dockerfile`, which did not
exist; and the `.env.local` it referenced had no reason to exist either. Three messages can
explain that — a scaffold makes it not happen.

It refuses to write over an existing project, which is the one case where clobbering is
unforgivable.

## `axon auth <manifest> [--lang ts]`
The verifier for what `[auth]` declares: standard JOSE, driven by the manifest — the
issuers, the key set, the accepted algorithms and the name of every claim. The same file
works against better-auth's JWT plugin, Auth0, Keycloak or Cognito by changing the
**manifest**, not the code.

Emitted and not linked, like everything else here: a generated file somebody can read and
edit beats a dependency that hides which claim it trusted. It refuses where it would have
to guess — `verify = "introspection"` is the issuer's API and its credential, and
`verify = "adapter"` is you saying you bring your own.

## `axon catalog <sources> [--service <name>]`
The declared lists —currencies, statuses, reasons— as one repeatable migration: the table,
the upsert of every entry, and the **delete of what is no longer declared**. That last one
is what keeps the two lists the same: a value removed from the manifest has to leave the
table, or the code stops offering it and the database keeps accepting it.

`axon build` emits the other half: a union type where a value off the list does not
compile, the frozen table and a lookup. See [The catalog](./patterns.md#the-catalog-one-list-in-three-places).

## `axon rls <sources> [--target sql|pg_anon]`
Data access policies: per-row RLS and per-column masked views. It comes from crossing the
real schema (read from the migrations) with the manifest's `pii` fields.

The output goes in `sql-policies/<service>/R__rls.sql`, not among the migrations: a policy
is not a schema change, and `R__` —Flyway's repeatable prefix— is what makes regenerating
it re-apply it instead of failing with a checksum mismatch. The `local` target already
ships the job that applies it to every node.

For the policy to do anything, the application has to connect with a **non-superuser**
role (a superuser always skips RLS) and pin the tenant inside the transaction:

```sql
BEGIN;
SET LOCAL ROLE axon_app;             -- the role the migration creates
SET LOCAL axon.tenant = '<tenant uuid>';
...
COMMIT;
```

`SET LOCAL`, never a session `SET`: with a pooler in transaction mode, a value that
survives the connection returns the previous tenant's rows with no error.

With `--target pg_anon` it emits
[pg_anon](https://github.com/TantorLabs/pg_anon)'s sensitive dictionary instead of the
SQL: that is for making a masked **copy** (staging, support, third parties), not for
protecting the live query. The rule comes from the column's type, and the framework's
tables are excluded from the dump because the `outbox` carries payloads.

## `axon load <manifest> [--check <summary.json>]`
Without `--check` it emits a [k6](https://k6.io) script: one scenario per HTTP route, at
the rate its `rate_limit` declares, with the p95 bounded by its `timeout_ms`. The
thresholds come from the manifest, not from a round number.

With `--check` it reads `k6 --summary-export`'s summary and gives the verdict: which
threshold was breached, how much traffic was measured, and whether it is getting close to
the ceiling the declared pool imposes. A summary with no thresholds is not a verdict, and
it says so.

## `axon analytics <sources> [--target bigquery|snowflake|clickhouse|plan]`
The warehouse's schema —one table per event, partitioned by date— plus the funnel views
that come out of the declared causal chain, in the dialect of each warehouse.

`--load <log>` emits the local target's loader instead of the schema: it carries the
envelope log into ClickHouse, filtering what is already loaded so running it twice does
not duplicate rows. `--introspect` emits the query that dumps the warehouse's REAL
schema, and `--check <dump>` diffs that dump against the manifest: a new field the table
does not have loads as nothing and nobody sees an error. `--vector` emits the Vector
config, which is the ingest path for a cluster where there is no managed warehouse to
subscribe to.

## `axon pooler <sources> [--service <name>] [--target local|...] [--users]`
[pgdog](https://pgdog.dev)'s configuration: the nodes, and the sharding derived from the
real schema —which tables carry the key and what type it is. One file per service,
because `[general]` is a singleton table. `--users` emits the `users.toml`, which pgdog
reads as a separate file.

# Infrastructure

## `axon infra <sources> [--target local|gcp|aws|k8s|plan] [--env <name>]`
Produces the neutral plan and renders it. The plan covers the edge (routes, auth, rate
limit, timeouts), the messaging (topics, subscriptions, DLQ), the compute, the state, the
buckets with their CDN, the secrets, the crons for the saga sweep and the snapshot prune,
the warehouse's ingest path and the OpenTelemetry variables.

OTel's resource attributes come from the manifest (`owner`, `tier`, `version`) and the
sampling from the `tier`: tier 0 is traced whole. `local` brings the backend up and traces
everything regardless of the tier; the other targets export to the endpoint you declare.
`--env` applies the deltas of `[env.<name>]` over `[infra]`. A non-native `--target` looks
for `axon-infra-<target>` on the `PATH`.

`--target plan` prints the plan in JSON: the escape hatch for any provider with no target.

## `axon ci <manifest> [--target gcp|aws|k8s] [--forge github|gitlab]`
A pipeline for GitHub Actions or GitLab CI. axon knows the **gates**: `verify` against every manifest (not
just its own), generated code up to date, migrations in dry-run with the right naming
convention, OIDC instead of keys, and infra applied before code — the topic has to exist
by the time the first pod that publishes to it starts.

The **deploy** comes from the `--target`, same as the infrastructure: Cloud Run, ECS or
`kubectl rollout`. Without `--target` it generates only the gates and fails at the deploy
step with a note: that part your team knows, not axon.

The **repo layout** comes from `[ci]` in `axon.policy.toml`, where `{service}` is
substituted with the service's name:

```toml
[ci]
manifests_dir  = "manifests"
service_dir    = "services/{service}"
test_cmd       = "make -C services/{service} test"
contracts_path = "services/{service}/src/contracts.ts"
image          = "${{ vars.REGISTRY }}/{service}@${{ steps.imagen.outputs.digest }}"
```

`image` is the one field that is not portable between forges: GitHub's `${{ }}` is
literal text for GitLab, so the deploy would push an image whose tag is the expression
itself and nothing would say so until somebody read the registry. Leave it out and each
forge gets its own default (`$CI_REGISTRY_IMAGE/{service}@$DIGEST` on GitLab); set it to
GitHub syntax and `--forge gitlab` **refuses** instead of emitting a broken pipeline.

The gates do not change with the forge —they are axon's— only the syntax around them:
`stages` and `rules` instead of `jobs` and `on`, `id_tokens` instead of
`permissions: id-token`, and `.axon` as the template job that installs the binary.

# Diagrams

| Command | Comes from | Gives |
| --- | --- | --- |
| `axon graph <sources>` | the manifests | event topology |
| `axon classes <sources>` | the manifests | class diagram |
| `axon er <sources>` | the migrations | entity-relationship |
| `axon states <sources>` | `[machine.*]` | the domain's state machines |
| `axon seq <event> <sources>` | the declared causal chain | the expected sequence |

They all emit Mermaid. GitHub renders them inside a ` ```mermaid ` block.

The schema for `er` and for the cross-service FK check is read with a PostgreSQL SQL
parser, not with regular expressions. A file that does not parse aborts the command with
the file and the error: axon prefers failing to guessing columns.

# Debugging

## `axon trace [log] [--correlation <id>] [--seq]`
Reads NDJSON envelopes (`-` or nothing = stdin) and reconstructs the real causal chain.
Without `--seq` it prints the tree; with `--seq`, Mermaid to diff against `axon seq`.

## `axon cap <sources> [-s <service>]`
Reconciles the declared CAP side with the patterns in use. It does not repeat what
`verify` blocks: it explains the consequences. `x` contradicts and `verify` blocks it, `!`
is a cost you pay, `i` is a consequence worth knowing.

`-s` narrows the report to certain services; the analysis still looks at all of them,
because without the others there is no way to know a dependency is AP.

## `axon flags <sources>`
[flagd](https://flagd.dev)'s configuration derived from the declared `[flags.*]`. The
gradual rollout is expressed with its `fractional`, pinned by the `sticky_by` field.

The `local` target brings flagd up with this configuration, and the generated code emits
typed accessors plus an interface with OpenFeature's shape — so the real SDK fits with no
translation layer.

# Colours

Blue informs, yellow warns, red blocks. They turn themselves off when the output is not a
terminal; `NO_COLOR` disables them and `CLICOLOR_FORCE` forces them.

# Verification

## `axon baseline <sources>`
A JSON snapshot of the published contracts: each event's schema with its owner, and each
method's signature. Save it as `axon.baseline.json` next to the manifests and commit it.
It is regenerated **on publishing**, not on every change.

## `axon verify <sources>`
Every rule in the README, plus `axon.policy.toml` if it sits next to the first source,
plus `axon.baseline.json` if it is there, plus every `axon-check-*` on the `PATH`. Exits
with 1 if there are errors.

With no baseline it cannot detect an incompatible change to an already published version,
and it says so as a warning instead of staying quiet.
