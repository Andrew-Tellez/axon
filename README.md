<h1 align="center">axon</h1>

<p align="center">
  <em>The manifest is the source of truth. The code, the infrastructure and the
  diagrams are projections. <code>axon verify</code> fails when they stop agreeing.</em>
</p>

<p align="center">
  <a href="https://andrew-tellez.github.io/axon/"><strong>Documentation</strong></a>
</p>

<p align="center">
  <a href="https://github.com/Andrew-Tellez/axon/actions/workflows/ci.yml"><img src="https://github.com/Andrew-Tellez/axon/actions/workflows/ci.yml/badge.svg" alt="ci"></a>
  <img src="https://img.shields.io/badge/runtime%20dependencies-0-brightgreen" alt="zero dependencies">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT"></a>
</p>

```sh
curl -fsSL https://raw.githubusercontent.com/Andrew-Tellez/axon/main/install.sh | sh
```

One binary. No runtime, no Node, no Python, no JVM. It runs the same on your laptop and
in an empty CI container.

---

## The problem

Ask any team with twenty microservices: *who consumes this event, and what breaks if I
change a field on it?* The honest answer is "you have to read five repos".

Today's frameworks do not help, because they live inside one language (NestJS, Spring,
Micronaut) or they are a runtime you have to deploy and operate (Dapr). None of them
knows that the `order.placed@v1` a Go service emits is the same one a Kotlin service
consumes. That relationship exists only in the team's head, until somebody leaves.

## The idea

You declare the service once, and everything else is derived from it:

```
                     ┌─ axon build      contracts, base class, resilient clients
  asyncapi.yaml ─────┤                     (axon import)
                     ├─ axon test       testkit: contract, idempotency, machines
                     ├─ axon openapi    OpenAPI 3.1 for the whole platform
                     ├─ axon infra      IaC: local · gcp · aws · k8s
 manifest.toml ──────┼─ axon rls        per-row RLS and masked views
                     ├─ axon flags      flagd configuration (OpenFeature)
  source of truth    ├─ axon ci         pipeline: axon's gates, the target's deploy
                     ├─ axon load       load test with the manifest's thresholds
                     ├─ axon graph · classes · er · states · seq   diagrams
                     ├─ axon trace      the REAL chain: envelopes, OTLP or Jaeger
                     ├─ axon cap        what the CAP side you picked implies
                     ├─ axon versions   the API's versions and their maintenance cycle
                     ├─ axon rules      the loop: a metric, a condition, what it proposes
                     ├─ axon traffic    who calls what, from the edge's log
                     ├─ axon pact       a pact from a consumer that does not use axon
                     ├─ axon tui        the system drawn, and what changed
                     └─ axon verify     drift: fails in CI
```

None of that is edited by hand. If the diagram does not match the code, it is not that
the diagram is stale: somebody broke the manifest, and CI says so before the merge.

> **The manifest is high-level design** —service boundaries, topology, what each one
> guarantees— **and the compiler lowers it into low-level design**: isolation level,
> retry policy, method signatures, infrastructure resources. And it verifies that they
> stay in agreement.

## The demo, in two commands

`examples/` ships three services that really run —one over four Postgres nodes with
[pgdog](https://pgdog.dev) in front, and another coordinating a saga. `./demo.sh` brings
the whole system up and makes **53 checks against reality**:

```console
$ cd examples && ./demo.sh
==> the real causal chain
└─ POST /v1/tenants/{tenantId}/orders <- http
   └─ order.placed@v1 <- orders
      └─ payment.captured@v1 <- payments

==> the trace in OpenTelemetry
  OK: 5 spans, one root, no orphans, crossing ['orders', 'payments']

==> expected (manifest) vs real (envelope log)
  OK: the system does exactly what it declares

==> the registry, from what is RUNNING
  OK: 3 services discovered live, and they declare what the repo says

==> tenant isolation through the pooler
  OK: pgdog rejects the query with no tenant at the router
  OK: 20 of 20 connections saw 1 row of their own and 0 of the other tenant's

==> the saga: compensation and resume, measured
  OK: the charge was undone and the merchant was not paid
  OK: resumed from the journal, compensated, and the refund reached the charge

==> event sourcing and CQRS, measured
  OK: two writes at the same version: one got in, the UNIQUE rejected the other
  OK: the view's lag is within the declared budget
  OK: the relay came back and published what was pending; nobody retried by hand
  OK: the snapshot says the same as the projection, which was built without it
  OK: a rolled-back transaction leaves no loose event: 0 payments, 0 events
  OK: with no snapshot at all the system stays correct, it just rebuilds more
  OK: the dirtied view is rebuilt from the stream, with the stream's dates
  OK: 24 reads during the rebuild and nobody saw a half-built view

==> declared vs occurred retries
  OK: 3 calls = 1 + 2 retries, exactly what was declared
  OK: 14000ms inside the 60000ms budget

==> the declared failures, measured
  OK: 1 call. The declared 2 retries were NOT spent on a failure that cannot end differently
  OK: 3 calls = 1 + 2 retries. Same policy, and the declaration is the only difference
  OK: order_rejected with 422, the status and the code the manifest declares

==> two versions of the same endpoint, and the retirement of the old one
  OK: it still answers 200 and goes out with the declared Deprecation and Sunset
  OK: the Link points at the v2, so nobody has to guess where to go
  OK: the v2 answers the same plus the customer, and announces nothing: it is the current one

==> las reglas declaradas, evaluadas contra la bodega
  OK: propone mover el flag declarado a la variante declarada, y volver a off al levantarse
  OK: no repite. Sin cooldown propondria lo mismo cada ventana, y lo que se repite se ignora
  OK: la guarda la frena. Con una sola metrica esto seria la ley de Goodhart con un cron

==> declared vs applied rollout
  declared 10%  measured 10.7%  (32 of 300)
  OK: stable per tenant, and the percentage applies

==> the warehouse: schema, funnel and PII
  OK: 12 flows, 12 reached the charge (100% conversion)
  OK: 12 orders and 26000 cents, the same as counting the table by hand
  OK: 12 hashed, 0 addresses in plaintext

==> declared vs measured capacity
  axon: 0 thresholds breached
```

It runs in CI on every push. The whole run —22 sections, 53 checks— and what happens when
you run it **twice in a row** are in
[The demo, measured](https://andrew-tellez.github.io/axon/demo.html).

## What it is built with, and what verifies it

**axon needs nothing to run**: one Rust binary, no runtime. The tools below are the ones
used by what it *generates*, and each one only for its own part — if one is missing,
that part is skipped and not the rest.

The list matters for one reason: **a generator is not validated with its own asserts, it
is validated with the real tool of its ecosystem.** The first three generators in this
project produced invalid output and the suite did not see it, because axon was only ever
verified against itself.

| Tool | What for | How the generated output is verified |
| --- | --- | --- |
| **Docker** | `--target local`: broker, Postgres per service, MinIO, Jaeger, flagd, the edge and your services | `./demo.sh` brings the system up and makes 53 checks against reality |
| **Terraform** | `--target gcp` and `--target aws` | `terraform validate` with the **real providers**, and with no warnings |
| **`tsc`** | the TypeScript from `axon build` and `axon test` | `tsc --strict --noEmit`, plus the example service's typecheck |
| **Node 24+** | runs the testkit with no build step, using type stripping | `node --test` against the real example service |
| **Go** | `axon-gen-go`, the reference plugin generator | `go vet` over what it emits, and `go/format` before emitting |
| **Postgres** | migrations, RLS, masked views | the RLS is **applied to a real Postgres** and checked to see that it isolates |
| **`kubectl`** | `--target k8s` | a parse of the 16 objects it emits |
| **k6** | `axon load`: load with the manifest's thresholds | it runs in the demo, and `--check` diffs the measured against the declared |
| **OpenTelemetry** | traces; the envelope already propagates `traceparent` | the demo verifies the span tree: one root, zero orphans, two services |
| **OpenFeature / flagd** | `axon flags`: evaluation over OFREP | the demo measures the declared rollout against the applied one |
| **Flyway** | applies the migrations; axon reads them, it does not run them | `validateMigrationNaming` mandatory: it used to skip files in silence |
| **BigQuery / Snowflake / ClickHouse** | `axon analytics`, and the ingest that feeds it | the DDL is parsed with **each one's dialect**, and the demo loads real events into ClickHouse and checks the funnel |
| **pgdog** | `axon pooler`: pooler and sharder, brought up by the local target | the `pgdog.toml` is validated against its **official JSON Schema**, and the demo measures tenant isolation through the pooler |
| **cocogitto** | Conventional Commits and the changelog | the hook rejects the message before the commit is created |
| **mdBook** | this documentation | every `toml` block in the pages goes through `axon verify` |

### Inside the binary

Seven dependencies, none of them accidental:

| | |
| --- | --- |
| `clap` | the CLI |
| `serde` + `toml` + `serde_json` + `serde_yaml_ng` | the manifest, and AsyncAPI in JSON or YAML |
| `indexmap` | insertion order: without it the generated output changes between runs and `git diff --exit-code` stops meaning anything |
| `sqlparser` | the schema comes out of the migrations with a real SQL parser. A regex breaks on `PARTITION BY` — and the worst part is that it breaks **in silence** |
| `ureq` | `axon discover` against live services |
| `ratatui` | `axon tui`: the only one that costs — 86 → 140 crates in the tree and 4.25 → 4.55 MB of binary. It was measured before being accepted, and everything else runs with the terminal untouched |

`regex` was here and left: it was down to checking three digits and an underscore in a
file name.

## Documentation

**[andrew-tellez.github.io/axon](https://andrew-tellez.github.io/axon/)** — built with
[mdBook](https://rust-lang.github.io/mdBook/), with search and an archived version for
every release. It is written in Spanish for now.

| | |
| --- | --- |
| [Your first manifest](https://andrew-tellez.github.io/axon/getting-started.html) | Ten minutes, from zero to verified |
| [The demo, measured](https://andrew-tellez.github.io/axon/demo.html) | The 53 checks against real containers, and what running it twice proves |
| [Architecture](https://andrew-tellez.github.io/axon/architecture.html) | High and low level design, in diagrams: the modules, and one declaration rendered on four targets |
| [Manifest reference](https://andrew-tellez.github.io/axon/manifest.html) | Every field and why it exists |
| [Patterns](https://andrew-tellez.github.io/axon/patterns.html) | Declared, not remembered: outbox, idempotent inbox, **saga**, **event sourcing**, **CQRS** |
| [CAP and resilience](https://andrew-tellez.github.io/axon/cap.html) | The side you do get to choose |
| [Rules and drift](https://andrew-tellez.github.io/axon/verification.html) | Everything `verify` blocks |
| [Security](https://andrew-tellez.github.io/axon/security.html) | OWASP, RLS, masking |
| [Plugins](https://andrew-tellez.github.io/axon/plugins.html) | Any `axon-*` executable |

The manifest examples in that documentation **are not text**: the suite extracts every
block and runs `axon verify` over it, so they cannot go stale in silence.

## Status

Preview. The command surface is stable; the manifest format can still change before
`v1`. See the [changelog](CHANGELOG.md) and
[what gets checked](https://andrew-tellez.github.io/axon/guarantees.html).

## Development

```sh
cargo test --release            # the whole suite
cargo run --release -- verify examples
cd examples && ./demo.sh        # needs Docker
mdbook serve docs --open        # the documentation
```

[Contributing](CONTRIBUTING.md) · [Design and decisions](DESIGN.md) · [gof-patterns](https://github.com/Andrew-Tellez/patterns) · MIT
