# One command each

Every capability, one line apiece, in the order you would actually reach for them. All of
them run against `examples/` in this repo — that is a system that really comes up, so
none of this is a snippet.

## Getting in

```sh
axon import asyncapi events.yaml > manifests/shipping.toml   # an event catalogue
axon import openapi swagger.json > manifests/billing.toml    # what a NestJS repo has
axon baseline manifests/ > axon.baseline.json                # freeze what is published
axon accept manifests/ > axon.accepted.json                  # the line: today's warnings
```

## The one you run every time

```sh
axon verify manifests/
```

Drift between what is declared and what is written: an event nobody consumes, an FK
crossing a service boundary, a retry against something not idempotent, a metric over
history that was deleted, a scope with a typo. It exits 1 on errors, and with
`axon.accepted.json` present a **new** warning fails too.

```sh
axon lsp   # the same findings as diagnostics, over stdio
```

The manifests are TOML, so the editor already highlights them; what it cannot see is
the half that lives between files. Point the editor's LSP client at `axon lsp` for
`*.toml` and it re-reads the workspace on open and on save — the report of `verify`,
placed on the line that caused it. It also completes: the keys of the block the cursor
is in, and the closed list of values a key accepts. Both come out of the model itself,
so a field that gets renamed renames in the editor. And it jumps: from a consumed event
to the `[emits]` that declares it, from a `[[depends]]` to the method block on the other
side — the jump the manifests are full of and that no editor can make on its own,
because it crosses files. Hovering an event says who emits it, who reads it and which of
its fields are personal data; hovering a service, what it promises. And it lists
references: every line of every manifest that names a method — which is the question
to ask before retiring one, and the one a grep answers wrong, because `getOrder` also
matches `getOrderV2`.

Any client that speaks LSP serves. In Neovim it is four lines, and the root is wherever
the platform's own policy lives:

```lua
vim.lsp.start({
  name = "axon",
  cmd = { "axon", "lsp" },
  root_dir = vim.fs.root(0, "axon.policy.toml"),
})
```

## The one an agent runs

```sh
axon verify manifests/ --json   # the same verdict, for whatever is not a person
axon mcp                        # the compiler as a tool, over MCP on stdio
```

A model is good at the part axon does not do —where the boundary goes, what an event is
called— and bad at knowing whether what it just wrote holds up, which is the part axon
answers. `axon mcp` exposes `verify`, `graph`, `manifest_schema` and `contracts` to any
MCP client; `manifest_schema` is the one that stops it guessing, because it comes out of
the compiler's own model and lists every block, key and accepted value this version
really understands. The loop, and the traps, are in
[AGENTS.md](https://github.com/Andrew-Tellez/axon/blob/main/AGENTS.md).

No model runs inside axon. A compiler that calls an API stops being deterministic, needs
a key, and cannot be the thing that settles an argument.

## What gets generated

```sh
axon build manifests/payments.toml manifests/     # contracts, base class, resilient clients
axon build http://payments:8080 manifests/        # ...from what the peer serves RIGHT NOW
axon build manifests/payments.toml manifests/ --lang go   # the same manifest, in idiomatic Go
axon test manifests/payments.toml manifests/      # testkit: doubles, fixtures, three suites
axon openapi manifests/                           # OpenAPI 3.1 for the whole platform
axon openapi manifests/ --api-version 2026-01-15  # ...as it was at that version
axon infra manifests/ --target local              # docker compose: broker, dbs, edge, warehouse, BI
axon infra manifests/ --target gcp                # terraform; also aws, k8s, or plan
axon infra --schema                               # the plan's JSON Schema, for a plugin
axon ci manifests/payments.toml --target gcp      # the pipeline, with axon's own gates
axon ci manifests/payments.toml --target gcp --forge gitlab   # the same gates for GitLab
axon init billing                                 # a project that verifies clean and comes up
axon auth manifests/orders.toml                   # the verifier, from the declared claims
axon crud manifests/shop.toml --expand            # what a [crud.*] stands for, to paste and edit
axon catalog manifests/ --service orders          # the declared lists: table, seed and type
axon rls manifests/                               # per-row RLS and masked views
axon pooler manifests/ --service orders --target local   # pgdog, from the manifest
axon flags manifests/                             # flagd config (OpenFeature)
axon analytics manifests/ --target clickhouse     # warehouse schema, funnels, metrics, retention
axon analytics manifests/ --metabase              # the connection and one question per metric
axon load manifests/orders.toml                   # k6 with a ramp into the declared limit
```

## The pictures

```sh
axon graph manifests/      # the event topology, as Mermaid
axon classes manifests/    # the class diagram
axon er manifests/         # entity-relationship, from the migrations
axon states manifests/     # the declared state machines
axon seq order.placed@v1 manifests/ --events      # the causal chain the manifest implies
axon tui manifests/        # the whole thing, drawn and animated
```

## What is actually happening out there

```sh
axon discover manifests/ http://orders:8080 http://payments:8080   # the repo vs what runs
axon trace .axon/log/local.ndjson                    # the real chain, from the envelopes
axon trace jaeger.json --manifests manifests/        # ...or from OTLP/Jaeger, plus the real edges
axon traffic manifests/ --check edge.ndjson          # who still calls the version being retired
axon pact manifests/ --check pacts/mobile.json       # a consumer that never adopted axon
axon analytics manifests/ --introspect               # the query that dumps the real schema
axon analytics manifests/ --check dump.tsv           # ...and the diff against the manifest
axon rules manifests/ --check windows.tsv            # what the declared rules propose
axon rules manifests/ --check windows.tsv --apply .axon/flags.json   # ...and move the lever
axon load manifests/orders.toml --check summary.json # declared capacity vs measured
```

## The ones that explain instead of blocking

```sh
axon cap manifests/ -s payments   # what the CAP side you picked costs you
axon versions manifests/          # the API's maintenance cycle, and what changed where
```

## The pattern behind all of it

Everything that touches a live system follows the same rule: **axon emits, somebody else
runs it, and the compiler diffs what came back.** No credentials for the warehouse, none
for the BI tool, none for the edge — the query, the log or the pact arrive through
`--check`. It is why the same binary works on a laptop and in a pipeline with no secrets
of its own.
