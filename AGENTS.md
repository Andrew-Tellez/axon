# Using axon as an agent

axon is a compiler. You design the system; it refuses the designs that
contradict themselves. Nothing here calls a model: what it gives you is a
vocabulary and a verdict, and both are worth more than a summary of them.

Install it, or run the server: `axon mcp` speaks MCP over stdio and exposes
`verify`, `graph`, `manifest_schema` and `contracts`. Without MCP the same
things are `axon verify --json`, `axon graph` and `axon build`.

## The loop

1. **Ask what can be declared.** `manifest_schema` (or read
   [the reference](docs/src/manifest.md)) lists every block, its keys and the
   closed list of values each key accepts. It comes out of the compiler's own
   model, so it is what *this* version understands. Guessing a key gets it
   refused: an unknown key is an error, not a warning.
2. **Write ONE service.** Not the platform.
3. **`verify` it**, fix what it says, and only then write the next one. A
   platform written in one go and verified at the end has to be unpicked all at
   once, and the findings arrive in a pile that hides which was the first
   mistake.
4. **Generate.** `axon build` for the contracts, `axon infra` for the
   infrastructure, `axon ci` for the pipeline. Never write those by hand: they
   are projections, and a projection somebody edited is drift with extra steps.

## Reading the findings

An **error** is a refusal: it does not compile and it does not deploy. A
**warning** is a decision somebody has to take, and with `axon.accepted.json`
present a new one fails the build too.

They are written as sentences that say what breaks and why —often with the
mechanism spelled out— because they are meant to be acted on. Do not summarise
them to the user as "3 warnings": say which, and what each one implies.

Two that are not about the manifest: a missing `Dockerfile` is about the repo,
and a missing `axon.baseline.json` means `verify` cannot see a breaking change
yet. Generate it with `axon baseline` once the contracts are published.

## What axon will not tell you

Where the service boundary goes, what an event should be called, which
guarantee the domain needs. That is the design, and it is why you are here.
What it *will* tell you is when the design says two things at once: a `strong`
consistency with a read replica, an event nobody consumes, a retry against
something that is not idempotent, a metric over a field the emitter does not
send.

## Things that are easy to get wrong

- **The event name carries its version**: `order.placed@v1`. A change that
  breaks its shape is a new event, not an edit.
- **`[cap]` is not documentation.** The declared side decides the isolation
  level, whether a replica can be read and what happens on a partition, and
  `verify` compares it against the topology that is actually declared.
- **The schema is read, never declared.** Tables come from the migrations,
  parsed with a real SQL parser. There is no `schema.toml` to keep in sync.
- **`uses` is what a consumer reads** of an event, not the whole payload.
  Declaring it is what lets a field the consumer does not read change freely.
- **An external service goes in a `*.external.toml`** with `external = true`.
  Its contract is frozen, not compiled.

## Where to look

`docs/src/manifest.md` is the reference, `docs/src/tour.md` is one example per
command, and <https://andrew-tellez.github.io/axon/playground.html> runs the
compiler in the page: two services, four buttons that break one rule each.
