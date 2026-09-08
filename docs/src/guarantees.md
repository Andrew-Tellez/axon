# What gets checked

The generators are validated with the real tool of the ecosystem, not with asserts of
their own — a compiler that only verifies itself produces invalid output:

| | |
| --- | --- |
| The generated TypeScript | `tsc --strict --noEmit` |
| The generated Terraform | `terraform validate` with the real providers (gcp and aws), with no warnings |
| The generated workflow | a YAML parse, the scalar blocks, and that no target leaks another cloud |
| The generated testkit | `node --test` against the real example service, and its failure suite catches a hand-edit of the generated `problem()` |
| The generated Go | `go vet` |
| The DDL | `PARTITION BY`, table-level constraints, and a loud failure on invalid SQL |
| The generated RLS | it is applied to a real Postgres and checked to see that it isolates |
| The generated pgdog config | validated against pgdog's official JSON Schema |
| The generated Vector config | `vector validate` in its own container |
| The warehouse schemas | parsed with each dialect's own parser |
| The declared metrics | their SQL parses in the three dialects, and the demo compares each one against counting the table by hand |
| The declared failures | the retriable codes land in the caller's client and the final ones do not, and the demo counts the calls each one really costs |
| The drawing | `axon tui --frames` renders through ratatui's test backend, and a test reads the picture: every service in it, the external one told apart, the CAP side, and the dying dependency marked |
| Who calls what | the demo generates traffic against the deprecated version and reads it back from the edge's real access log |
| A foreign pact | crossed against the declared contract: a field nobody returns fails, and the ones the consumer does not read get named |
| The BI provisioning | the demo provisions a Metabase from zero and compares a question against the same view read from ClickHouse: the dashboard and the manifest have to answer the same number |
| The rules over a metric | their SQL parses in the three dialects, and the demo seeds a falling history in ClickHouse to check that it proposes once, does not repeat, and stays quiet when the guard falls |
| A split manifest | splitting one in two has to produce byte-identical output from `axon build` |
| Declared consumption | reading a field nobody declared does not compile: `tsc --strict` refuses it against the generated type |
| The generated double | `node --test` over `FakeTransport`: a retriable failure arrives 1 + retries times and a final one exactly once, with no network |
| The version adapters | `tsc --strict` over the chain and `node --test` running it: the oldest shape has to come out of two adapters applied in order |
| A job on the four targets | its HCL goes through `terraform validate` with the real providers, and its k8s YAML through a parse: a CronJob's extra nesting is exactly where an indentation breaks |
| The four targets | they deploy the workload and deliver to somebody |
| The book's manifest examples | every ```toml block goes through `axon verify` |
| The output the book quotes | four consecutive words of it have to appear in `src/` |
| The demo output the book quotes | four consecutive words of it have to appear in the script that prints it |
| The architecture page | every module in `src/` is in its map, and every provider resource it names is really emitted by that target |
| The diagrams and the registry | the emitter's edge, the consumer's edge and the synchronous call are in them, and an external service is told apart |

```sh
cargo test --release      # 62 conformance checks
cd examples && ./demo.sh  # 51 checks against real containers
```

Preview. The command surface is stable; the manifest format can still change before
`v1`.

Deliberately skipped, and when to add it:

- **One native generator (TS)** — the rest through plugins, until there is a second real
  service in another language that justifies bringing it into the core.
- **One SQL dialect (PostgreSQL)** — the schema is read with `sqlparser`, not with a
  regex, so it holds up to `PARTITION BY`, table-level constraints and whatever any ORM
  generates. A file that does not parse is an error, never a silence.
- **One execution model (`container`)** — any other value of `runtime` is a `verify`
  error, not a field ignored in silence. Another model comes in through an
  `axon-infra-*`.
- **`verify` compares declarations, not the deployed cloud** — drift against Terraform's
  state arrives when there is something deployed to verify. `axon analytics --check`
  already does this shape of thing for the warehouse.
- **No runtime of its own** — `Bus`, `Inbox` and `Outbox` are one-line interfaces; the
  adapter belongs to whoever deploys. A runtime package when the same adapter shows up in
  three services.
