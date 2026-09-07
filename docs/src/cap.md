# CAP and resilience

The manifest is high-level design — service boundaries, topology, what each one
guarantees. The compiler lowers it into low-level design: isolation level, retry policy,
method signatures. **That is the whole project in one sentence.**

```toml
[[depends]]
service    = "orders"
method     = "getOrder"
timeout_ms = 1000        # mandatory
retries    = 3           # only if the other method is idempotent
breaker    = true

[cap]
consistency  = "strong"    # money does not tolerate a stale balance
on_partition = "reject"    # rather than serve something stale, it serves nothing
```

`axon build` emits the client with that policy running: timeout, exponential backoff
**with full jitter** (without jitter every client retries at the same instant and the
other side never comes back up), and one breaker per target that goes half-open after
the cooldown. Retries are only emitted for idempotent methods — `verify` blocks the rest
— and the call carries `traceparent`, `x-correlation-id`, `x-causation-id` and
`idempotency-key`.

The retry budget is also spent according to the callee's declared failures: the client
carries the codes that method declared as `retriable` and does not try again on the rest.
See [`errors` on a method](./manifest.md#errors-on-a-method).

## `axon cap`: reconciling what you declared with what you use

`verify` blocks the contradictions. `axon cap` explains the **consequences**, which is a
different thing: there are combinations that are not an error and still change what the
service can promise.

```console
$ axon cap manifests/ -s payments
payments  [CP]  consistency = strong, on_partition = reject
  x dependency         contradicts  `orders`, which is AP, is called on a synchronous
                                    path: the path's guarantee is the weaker one
  ! saga               costs  `refund` compensates an earlier step. A compensation is
                              eventual consistency by construction: your own state is
                              CP, the FLOW is not
  i outbox             implies  your own state stays consistent, but consumers see it
                                late: the relay publishes after the commit
  i standby HA         implies  it breaks no consistency: nobody reads from the standby,
                                it only takes over. It is the only thing on this list
                                that improves availability at no cost in the C
```

**`x` is what `verify` blocks, `!` is a cost you pay, `i` is a consequence worth knowing
before an incident.** The `-s` filter narrows the report, but the analysis still looks at
every service: without `orders` loaded there would be no way to know that dependency is
AP.

## The side of the theorem you do get to choose

Partition tolerance is not an option: the network partitions. What you choose is what to
do while it is partitioned, and that decision **changes the code**:

| | `strong` / `reject` (CP) | `eventual` / `degrade` (AP) |
| --- | --- | --- |
| `isolationLevel` | `SERIALIZABLE` | `READ COMMITTED` |
| Staleness | — | `maxStalenessMs`, mandatory |
| The client's signature | `(input, e)` | `(input, e, fallback)` |

The last row is the one that matters: if you declare `degrade`, the generated client
**requires** a `fallback` parameter. You cannot say "I choose availability" and then not
write what gets served when the other side is not there. It is not a convention — it
does not compile.

`verify` blocks `strong` + `degrade` (it is the theorem's contradiction), requires
`max_staleness_ms` on everything `eventual` (without a number, "eventual" is a word), and
warns when a `strong` service synchronously calls an `eventual` one: **the path's
guarantee is the weakest link's, not yours.**
