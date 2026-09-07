# axon

A **language-agnostic** backend framework for microservices and event-driven
architectures. One manifest per service is the source of truth; the code, the
infrastructure, the contracts and the topology are projections of it.

```
manifest.toml ─┬─ axon build --lang ts   → contracts + base class (envelope, handlers, emitters)
               ├─ axon infra             → IaC: local · gcp · aws · k8s (topics, subs, DLQ, DB, secrets)
               ├─ axon graph             → mermaid: event topology
               ├─ axon classes           → mermaid: class diagram
               ├─ axon er                → mermaid: entity-relationship (from the migrations)
               ├─ axon seq <event>       → mermaid: the expected causal flow
               ├─ axon discover          → registry of services and their methods (local or live)
               └─ axon verify            → drift: fails in CI when they stop agreeing
```

## Diagrams

None of them is drawn by hand and none of them introduces a new source of truth:

- **Classes** (`axon classes`) — a direct projection of the manifest: services, events as
  classes, handlers, emitters, synchronous dependencies, and which patterns each one
  implements.
- **ER** (`axon er`) — **introspected** from the migrations, not declared. Putting columns
  in the manifest would be the dual-write problem dressed up as documentation. The
  manifest only adds what the migrations do not know: which service each table belongs to.
- **Sequence** (`axon seq order.placed@v1`) — walks the declared causal chain: who
  consumes, who they call, what they emit afterwards. It is the *expected* flow; the
  `causationId` of the real envelopes says the one that occurred. `axon trace --seq`
  prints that one, and diffing the two is a drift check you can run against a live system.

## Why it exists

The existing frameworks are libraries inside one language (NestJS, Spring, Micronaut) or
a runtime you have to deploy (Dapr). None of them answers "who consumes this event and
what happens if I change a field on it?" without reading code from five repos. axon
answers it because that relationship is declared, not inferred.

Three things are not optional, and that is why the compiler generates them instead of
leaving them to the team's discipline:

1. **Traceability from day one.** Every message travels in a CloudEvents envelope
   extended with `traceparent` (W3C), `correlationId` (stable across the whole business
   flow) and `causationId` (the message that caused it). The generated emitter receives
   the causing message and propagates the chain: there is no way to publish an orphan
   event without stepping outside the framework.
2. **Discovery.** Each service serves its own manifest at `/.well-known/axon.json`.
   `axon discover <dir|url>` merges manifests from disk and from live services into a
   registry with methods, inputs, outputs and events. External services (Stripe, an ERP)
   are frozen into a `*.external.toml` — discovered once, then versioned as a contract.
3. **Infrastructure as code.** The `[infra]` block and the declared events produce the
   IaC: one topic per event, one subscription per consumer, a DLQ always, the service's
   database and its secret containers. There is no topic without an owner and no consumer
   without a DLQ, because there is no way to write one.

## Patterns: declared, not remembered

A pattern you have to remember to apply is not a pattern, it is a convention somebody
will break at 3am. In axon the pattern is declared and the compiler emits it; if it is
not in the generated code, it does not exist.

| Pattern | Declared with | What it generates |
| --- | --- | --- |
| **Transactional outbox** | `[patterns] outbox = true` | The emitters write into the outbox, not into the bus — the direct `publish` stops existing, and the caller's transaction is a mandatory parameter. The IaC creates the table and the relay's user. Goodbye dual-write. |
| **Idempotent consumer (inbox)** | always | `dispatch()` deduplicates by envelope `id` before routing. The broker delivers at least once; the effect happens exactly once. |
| **Envelope / causal chain** | always | `traceparent`, `correlationId`, `causationId` propagated by the generated emitter. |
| **Dead letter** | always | A subscription with a `dead_letter_policy` and its topic. There is no way to declare a consumer without a DLQ. |
| **Database per service** | `[infra] state` | One database per service, never shared. |
| **Versioned contracts** | `event@vN` | `verify` blocks a schema change on a published version. |
| **Saga** | `[saga.<name>]` | A coordinator that calls in order and, on a failure, undoes everything ATTEMPTED in reverse order; a journal, a resume sweep, and a time budget that has to cover the sum of the steps. |
| **Event sourcing** | `[aggregate.<name>]` | An append-only stream with a mandatory `UNIQUE (stream_id, version)`, a `fold` with one case per declared event, and snapshots as a cache carrying the rules version they were computed with. |
| **CQRS** | `[view.<name>]` | A projection with a per-stream checkpoint, a shadow-table rebuild, and a staleness budget that has to fit inside the service's. |

The GoF patterns live one level down, in the code the team writes — that is what
[`gof-patterns`](https://github.com/Andrew-Tellez/patterns) is for, in six languages.
axon does not reimplement them: it deals with the *architectural* patterns, the ones that
cross processes and that no library inside a single language can guarantee on its own.

## Migrations

axon is **not** a migration tool — Flyway, Alembic and golang-migrate already exist and
are better at it. What axon does is treat them as the schema's source of truth and verify
what they cannot see:

```toml
[infra]
migrations = "sql/payments/"
```

```
sql/payments/
  001_payment.expand.sql       expand:   additive, backwards compatible
  002_provider_ref.expand.sql  expand:   a new nullable column
  003_drop_legacy.contract.sql contract: destructive, and it says so in the name
```

- The schema is the sum of the migrations folded in order. There is no duplicated
  `schema.sql` to drift; the ER diagram comes from here.
- **Expand → migrate → contract** is mandatory, not a recommendation: a migration with a
  `DROP` that is not called `.contract.sql` is a `verify` error. Deploying a destructive
  one alongside the code that stops using the column breaks the rollback.
- **No FK crosses a service boundary.** `verify` blocks it: you store the id, and
  consistency between services is resolved with events, not with the database engine.
- A numeric prefix is mandatory, or the order is not deterministic. Two migrations
  sharing a version is an error too: Flyway applies NEITHER.

## Drift verification

`axon verify` is what turns the manifest into something more than documentation:

| Check | Result |
| --- | --- |
| An event is consumed that nobody emits | error |
| Two services emit the same event with different schemas | error |
| A method is depended on that the other service does not expose | error |
| An event is emitted with no consumers | warning |
| An FK crossing a service boundary | error |
| A destructive migration not marked `.contract.sql` | error |
| A migration with no numeric prefix | warning |
| A field changed on an already published version | error |

That table is the shape of it, not the whole of it: there are over a hundred rules, and
[the documentation](https://andrew-tellez.github.io/axon/verificacion.html) lists them.

In CI, run against the live manifests
(`axon verify https://orders/... https://payments/...`), it compares what is declared
with what is deployed.

## The manifest

```toml
service = "payments"
version = "1.2.0"

[emits."payment.captured@v1"]      # name@version, always
paymentId = "uuid"
amount    = "money"

[consumes."order.placed@v1"]
handler = "onOrderPlaced"

[methods.capturePayment]
in  = { orderId = "uuid", amount = "money" }
out = { paymentId = "uuid" }

[[depends]]
service = "orders"
method  = "getOrder"

[infra]
state   = "postgres"
runtime = "container"
secrets = ["STRIPE_API_KEY"]
```

Types: `string int float bool timestamp uuid json money`. `money` is a dedicated type on
purpose: a float for money is a bug waiting its turn.

## Conventions

- Events in the past tense and versioned: `domain.happened@vN`. An incompatible change
  is `@vN+1`, never an edit to a published version's schema.
- A service owns the events it emits. Nobody else emits them.
- Synchronous communication (`methods`) is declared the same way as asynchronous; if it
  is not in `depends`, the call should not exist.
- Generated code is not edited. You inherit from the base class and implement the
  abstract members.

## Status

Preview. One Rust binary with no runtime dependencies, four IaC targets (`local`, `gcp`,
`aws`, `k8s`), and a conformance suite that validates every generator with the real tool
of its ecosystem.

Deliberately skipped, and when to add it:

- **One code target (TypeScript)** — another language is another `*_ts`-shaped generator
  or an `axon-gen-<lang>` plugin, not a template engine. `axon-gen-go` exists as the
  reference plugin. Add a native one when there is a second real service in another
  language.
- **No runtime of its own** — the `Bus` is a three-line interface; the adapter belongs to
  whoever deploys. Add a runtime package when the same adapter shows up in three services.
- **`verify` compares manifests with each other and with the migrations, not with the
  cloud** — drift against terraform state or against the real topics arrives when there
  is something deployed to compare with. `axon analytics --check` already does this shape
  of thing for the warehouse: axon emits the introspection query, somebody runs it, and
  the compiler diffs the dump against the manifest.
