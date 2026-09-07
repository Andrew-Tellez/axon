# axon

> The manifest is the source of truth. The code, the infrastructure, the tests and the
> diagrams are **projections**. `axon verify` fails in CI when they stop agreeing.

One Rust binary, no runtime. Four infrastructure targets from a neutral plan. Mandatory
traceability by construction. Patterns enforced by generation, not by discipline.

```sh
curl -fsSL https://raw.githubusercontent.com/Andrew-Tellez/axon/main/install.sh | sh
```

Ask any team with twenty microservices: *who consumes this event, and what breaks if I
change a field on it?* The honest answer is "you have to read five repos". Today's
frameworks do not help, because they live inside one language (NestJS, Spring,
Micronaut) or they are a runtime you have to deploy and operate (Dapr).

None of them knows that the `order.placed@v1` a Go service emits is the same one a
Kotlin service consumes. That relationship exists only in the team's head, until
somebody leaves.

You declare the service once. Everything else is derived from it:

```
  asyncapi.yaml ─────┐                     (axon import)
                     │
                     ├─ axon build      code: contracts + base class
                     ├─ axon test       tests: unit, integration, e2e
                     ├─ axon openapi    OpenAPI 3.1 for the whole platform
                     ├─ axon infra      IaC: local · gcp · aws · k8s
 manifest.toml ──────┼─ axon ci         a pipeline with the gates that matter
                     ├─ axon graph      event topology
  source of truth    ├─ axon classes    class diagram
                     ├─ axon er         entity-relationship (from the migrations)
                     ├─ axon seq        the expected causal flow
                     ├─ axon trace      the REAL causal flow (local debugging)
                     ├─ axon discover   registry of services and methods
                     └─ axon verify     drift: fails in CI
```

None of that is edited by hand. If the diagram does not match the code, it is not that
the diagram is stale: somebody broke the manifest, and CI says so before the merge.
