# Declared scenarios — a proposal

**This page is a design note, not a feature.** Nothing here is implemented. It is written
down because the idea is good enough to be worth arguing with, and because the shape it
should NOT take is the interesting part.

## What it would be

A file that declares a situation — *place an order, make the payout fail, see what the
system does* — instead of the shell script that does it today.

`examples/` already has fourteen of those situations, and every one of them is a
hand-written script: `check-saga.sh`, `check-retries.sh`, `check-errors.sh`,
`check-versions.sh`, `check-rules.sh`. They are the most valuable thing in the repo —
they measure against real containers — and also the least declarative thing in it. That
is the tension this note is about.

## What it must not do

**A scenario must not declare what it expects to happen.** The causal chain of a flow is
already derivable: `axon seq order.placed@v1 . --events` prints it from the manifest, and
the demo diffs it against the real envelope log. If the scenario file restated that
chain, the manifest would stop being the source of truth for it, and the two would drift
exactly the way a mock drifts from the service it doubles.

So a scenario declares only what cannot be derived:

- **where the flow starts** — a declared method, with its input,
- **what fails**, if anything — a declared failure of a declared method,
- **what the data is** — amounts, ids, tenants: the business values.

Everything it asserts comes from the manifest.

```toml,no-verify
# scenarios/checkout.toml — a proposal, not a format
[scenario.the_payout_fails_and_the_charge_is_undone]
service = "checkout"
call    = "checkout"                       # a declared method
input   = { orderId = "$uuid", amount = { amount = 200000, currency = "MXN" } }

# A DECLARED failure of a declared method: `verify` can already tell that
# `merchant_ceiling` exists and that it is final, so it also knows the client
# will not retry it. A fault that names a code nobody declared is refused.
[[scenario.the_payout_fails_and_the_charge_is_undone.fault]]
service = "payments"
method  = "payoutMerchant"
fails   = "merchant_ceiling"

# What it ends in. The state machine says which states exist and which are
# final, so an unreachable one is refused before anything runs.
[scenario.the_payout_fails_and_the_charge_is_undone.expect]
machine = { payment = "refunded" }
saga    = "compensated"
```

## What could be refuted without running anything

This is what would make it worth declaring rather than scripting:

| | |
| --- | --- |
| A fault naming a failure the method does not declare | error |
| An entry point that is not a declared method | error |
| An expected state that the declared machine cannot reach | error |
| An expected saga outcome that its declared steps cannot produce | error |
| An expected chain that contradicts the one `axon seq` derives | error |
| A scenario declared and run by nobody | warning |

The last one matters more than it looks: a scenario that exists and is never executed is
a test that passes by not being there.

## What it would take to run one

The same shape as everything else that touches a live system: axon emits, somebody runs,
the compiler diffs. `axon scenario` would emit the calls and the fault switches; the
system already writes the envelope log, and `axon trace --seq` already reads it. The
comparison against the derived chain exists today — that is the demo's *expected
(manifest) vs real (envelope log)* step.

The hard part is not the running. It is the faults: today they are environment variables
the example's own services honour (`AXON_DEMO_PAYOUT_SLOW_MS`, `AXON_DEMO_RAIL_BUSY`).
Declaring a fault means the generated code has to have a place to inject it, and a
service that can be told to fail on demand in production is a hole with a nice name. The
honest answer is probably that faults only exist under `--target local`, and that a
scenario that declares one refuses to run against anything else.

## Why it is not built

Three reasons, in order of weight:

1. **The scripts work and are read as evidence.** They print numbers measured against
   containers. A declarative version has to be at least as convincing, and a green tick
   is less convincing than `3 calls = 1 + 2 retries`.
2. **The fault injection is a real design problem**, not a syntax problem. Getting it
   wrong puts a switch in production code.
3. **It would have to express what the demo already does**, or it is a toy. That is the
   bar: `check-saga.sh` and `check-errors.sh` should shrink to a scenario file each, and
   the output should stay as specific as it is now.

Until those three have an answer, a scenario file would be a second way to say things
axon already says — which is the one thing this project exists to avoid.
