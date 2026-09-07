# What gets checked

The generators are validated with the real tool of the ecosystem, not with asserts of
their own — a compiler that only verifies itself produces invalid output:

| | |
| --- | --- |
| The generated TypeScript | `tsc --strict --noEmit` |
| The generated Terraform | `terraform validate` with the real providers (gcp and aws), with no warnings |
| The generated workflow | a YAML parse, the scalar blocks, and that no target leaks another cloud |
| The generated testkit | `node --test` against the real example service |
| The generated Go | `go vet` |
| The DDL | `PARTITION BY`, table-level constraints, and a loud failure on invalid SQL |
| The generated RLS | it is applied to a real Postgres and checked to see that it isolates |
| The generated pgdog config | validated against pgdog's official JSON Schema |
| The generated Vector config | `vector validate` in its own container |
| The warehouse schemas | parsed with each dialect's own parser |
| The four targets | they deploy the workload and deliver to somebody |
| The book's manifest examples | every ```toml block goes through `axon verify` |
| The output the book quotes | four consecutive words of it have to appear in `src/` |

```sh
cargo test --release      # 57 conformance checks
cd examples && ./demo.sh  # 31 checks against real containers
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
