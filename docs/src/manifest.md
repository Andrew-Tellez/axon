# The manifest

```toml
service = "payments"
version = "1.2.0"
owner   = "payments-team"    # governance: nothing without an owner
tier    = "0"                # criticality: decides SLO and alerts

[emits."payment.captured@v1"]        # domain.happened@version, always
paymentId = "uuid"
amount    = "money"

[consumes."order.placed@v1"]
handler = "onOrderPlaced"

[methods.capturePayment]
http       = "POST /v1/payments"
idempotent = true                    # mandatory if it mutates
in  = { orderId = "uuid", amount = "money" }
out = { paymentId = "uuid" }

[[depends]]
service    = "orders"
method     = "getOrder"
timeout_ms = 1000                    # mandatory: with no budget, there is no call
retries    = 3
breaker    = true

[patterns]
outbox = true

[infra]
state      = "postgres"
migrations = "sql/payments/"
secrets    = ["STRIPE_API_KEY"]

[env.prod]                           # environments are deltas, not copies
min_instances = 3
```

Types: `string int float bool timestamp uuid json money`. `money` is a dedicated type on
purpose — a `float` for money is a bug waiting its turn.

The business logic is written by a person. Always. A manifest that tries to express
*all* the behaviour ends up being a new programming language, and a worse one than the
six it compiles to — that is exactly where MDA, Rational Rose and low-code died.

But there is a band that **is** identical in every language and today lives scattered
across `if`s: **the decision rules, not the implementation.**

```toml
[machine.payment]
initial = "pending"
final   = ["refunded", "failed"]

[machine.payment.transitions.capture]
from  = ["pending"]
to    = "captured"
on    = "capturePayment"           # the method that fires it
emits = "payment.captured@v1"      # the event that comes out on completing it

[machine.payment.transitions.refund]
from        = ["captured"]
to          = "refunded"
on          = "refundPayment"
compensates = "capture"            # the inverse, inside the same machine
```

`axon build` generates the exhaustive, typed transition table — `PaymentState`,
`paymentCan()`, `paymentNext()` which blows up on an illegal transition. `axon states`
draws it. And `axon verify` **proves properties over it** before the merge:

- a state unreachable from the initial one
- a non-final state with no way out — a deadlock, found in CI and not in production
- a transition fired by a method or event that does not exist
- a transition that emits an event the service does not declare it emits
- a compensation pointing at a step that does not exist

**The what is portable; the how is not.** The body of `capturePayment` — charging on
Stripe, deciding whether it fails, writing the row — is yours, in your language, in an
`extends` of the generated class. axon keeps the part that can be verified.

## `errors` on a method

```toml
[methods.payoutMerchant]
http       = "POST /v1/payouts"
auth       = "required"
idempotent = true
in  = { paymentId = "uuid", amount = "money" }
out = { payoutId = "uuid" }
errors = [
  { code = "merchant_ceiling", status = 409, detail = "over the merchant's ceiling" },
  { code = "rail_busy", status = 503, retriable = true, detail = "the rail is saturated" },
]
```

Declared like `in` and `out`, and for the same reason: how a method fails is part of its
contract. Today those failures live in the handler's body, where the caller cannot see
them, so every caller invents its own reading of a 500.

`retriable` is what makes this more than documentation — it **changes the generated
client**. A failure the callee declared as final is not retried at all: retrying a
declined card ends in the same answer and spends the caller's time budget on the way,
and in a saga that budget is what is left to compensate with. The default is `false`,
which is the honest one.

Three projections come out of the same declaration:

| Projection | What it gets |
| --- | --- |
| The callee's code | `declaredErrors` plus a `fail()` typed against the manifest: an undeclared code does not compile |
| The caller's client | the retriable code list `withPolicy` consults before trying again |
| `axon openapi` | one response per code, with `x-axon-code` and `x-axon-retriable` |

What `verify` refutes: a status that is not 4xx or 5xx, a code that is not `snake_case`,
the same code twice, `retriable = true` on a 4xx that is not 408, 425 or 429 — a 4xx says
the request is what is wrong, so sending it again ends the same — a public mutation with
no declared failure, and retries against a method whose every failure is final.

The demo measures it: the same route and the same policy, and the final failure arrives
**once** while the retriable one arrives `1 + retries` times. See
[The demo, measured](./demo.md).

## `[rules.<name>]`: a metric, a condition, and what it proposes

```toml
[rules.gmv_usd_falling]
metric   = "gmv"                          # a declared metric
where    = { "total.currency" = "USD" }   # the segment it applies to
compare  = "previous"                     # previous · same_day_last_week · absolute
below    = 0.85                           # 15% under the reference
for      = 2                              # consecutive windows: one bad day is not a trend
cooldown = 3                              # and three quiet ones before, or it says nothing
mode     = "propose"                      # the only mode there is

# The other direction, declared. Without it this is Goodhart's law with a cron:
# the lever moves the number it is judged by and nobody watches what it moves
# the other way. Here it says GMV is falling but the ORDER COUNT is not — so
# demand did not leave, the basket shrank, which is what free shipping moves.
[[rules.gmv_usd_falling.guard]]
metric = "orders_by_currency"
where  = { "total.currency" = "USD" }
above  = 0.9

[rules.gmv_usd_falling.then]
flag    = "free_shipping"     # a declared flag...
variant = "over_500"          #   ...on a declared variant
restore = "off"               # what goes up on its own has to come back down on its own
```

The loop that today lives in a dashboard alert plus a runbook nobody ran. The metric is
already declared, the flag is already declared and so is the event catalogue, so what was
missing was saying out loud **which condition on which metric leads to which of them**.

**It only ever proposes.** `mode = "apply"` is refused with its reason: writing to
production off a metric is a control loop, and that gets decided on its own, not in a
field. `axon rules` emits the SQL —axon has no warehouse credentials and does not want
them— and `axon rules --check <tsv>` takes the decision over the rows that come back:

```console
$ axon rules manifests/ --check windows.tsv
proposes  orders.gmv_usd_falling
  set `free_shipping` to `over_500` (back to `off` when it lifts)
  `gmv` = 64000 for total.currency = USD held the condition for 2 windows, the 3 before were quiet, and 1 guard held
axon: 1 of 1 rules propose a change; none was applied
```

Three things worth naming, because they are the difference between this and a cron with
a threshold:

- **It proposes on the way IN.** The `cooldown` is not state kept anywhere: it says the
  condition was NOT holding in that many windows before it started to. Same data, same
  answer, today and in a re-run — nothing to remember and nothing to get out of sync.
- **The window still filling is excluded.** Comparing half a day against a whole one
  makes every rule fire every morning and lift by itself at noon.
- **A segment is a dimension the event carries**, not a cohort kept elsewhere. That is how
  "this rule is only for this kind of customer" gets declared: the tier travels in the
  event, as it was when it happened, so the past does not get recomputed with today's
  classification.

What `verify` refutes: a metric or flag that does not exist, an unpinned dimension —a
grouped metric is one series per group, and unpinned it compares one group's number
against another's—, no `cooldown`, no `restore`, flipping a `kill_switch` (that switch is
a person's), two rules over the same flag, a guard that is the trigger written again, and
a rule with a lever and no guard at all, which it names for what it is.

## `include`: one service, split by feature

A service grows and its manifest with it. The blocks of a feature can live in their own
file:

```toml
service = "payments"
owner   = "payments-team"
tier    = "0"
include = ["payments"]        # a file, or a directory: every *.toml inside, sorted

[infra]
state = "postgres"
```

```toml
# payments/payouts.toml — a fragment, not a manifest: it carries no `service`
[methods.payoutMerchant]
http = "POST /v1/payouts"
auth = "required"
idempotent = true
in  = { paymentId = "uuid", amount = "money" }
out = { payoutId = "uuid" }
```

A fragment carries only what belongs to a **feature**: `emits`, `consumes`, `methods`,
`depends`, `machine`, `saga`, `aggregate`, `view`, `metrics`, `flags`, `pii`. What belongs
to the **service** — `[infra]`, `[cap]`, `[analytics]`, `[api]`, `[patterns]`, `[pooler]`,
`[env.*]` — stays in the manifest, and a fragment that declares one is refused by name
rather than merged: two `[cap]` blocks with different answers is not a split, it is a
contradiction.

Two rules hold the merge up. **Every collision is an error** naming both files — across
files, last-one-wins is drift nobody would ever see. And **the split is invisible**:
`include` is not serialized, so what the service serves at `/.well-known/axon.json` and
what the baseline records is the merged manifest. A test splits a manifest in two and
checks that what comes out of `axon build` is byte for byte the same.

## `uses`: what each consumer really reads

```toml
service = "payments"
owner   = "payments-team"
tier    = "1"

[consumes."order.placed@v1"]
handler = "onOrderPlaced"
uses    = ["orderId", "total"]     # of the four fields the owner declares

[[depends]]
service = "orders"
method  = "getOrder"
uses    = ["orderId", "status"]

[[depends]]
service = "payments"
method  = "refundPayment"
uses    = []                       # a declaration too: it reads nothing
```

This is the question a producer cannot answer on its own, and the reason a contract ends
up frozen: *somebody might be using it*. Declared, it has an answer.

And it cannot drift, which is what separates it from a pact recorded once: **the fields
nobody declared do not exist on this side**. The consumer's type of somebody else's event
is what it declared it reads, and the client's answer type likewise — a pact goes stale
the day somebody reads one more field, this does not compile. Absent means undeclared and
nothing is concluded; an empty list is the honest answer of a handler that only reacts.

What it buys:

| | |
| --- | --- |
| `verify` | using a field the provider does not return is an error — either it was renamed and the caller reads `undefined`, or the declaration is wrong |
| `verify` | a field **nobody** reads is a warning that says it can be removed. Only when every consumer declared: with one that declared nothing there is no answer, and saying "delete it" without one is how a field somebody was reading gets deleted. An exported event is exempt — the warehouse reads every field |
| `axon baseline` | removing a published field names **who** reads it, and when nobody does it drops to a warning: it is not a breaking change |
| The testkit | `FakeTransport` answers each dependency with a fixture of ITS contract, cut to what was declared. A hand-written mock can answer a field the other side does not return, and the test passes right up to production |

Same value Pact gets from recording traffic, without recording anything, and it fails in
the PR instead of after the deploy.

## `[api]`: how the API is versioned

Two schemes, and `verify` requires the whole platform to declare the same one — with
two, a caller has to know which service it is talking to before it can know how to ask
for a version.

**`"path"` (the default).** The version is the route: `/v1/...` and `/v2/...` are two
methods that coexist, and the old one declares its retirement.

```toml
[methods.getOrder]
http = "GET /v1/tenants/{tenantId}/orders/{orderId}"
out  = { orderId = "uuid", status = "string", total = "money" }
deprecated = "2026-09-01"     # RFC 9745, in the response
sunset     = "2027-12-31"     # RFC 8594: the day it stops being served
successor  = "getOrderV2"     # Link rel="successor-version"

[methods.getOrderV2]
http = "GET /v2/tenants/{tenantId}/orders/{orderId}"
out  = { orderId = "uuid", status = "string", total = "money", customerId = "uuid" }
```

Deprecated is not gone: the route keeps answering what it always answered and says so on
the way out, with the formats each RFC asks for — `Deprecation` is an sf-date in seconds
and `Sunset` an HTTP-date, neither of them the manifest's ISO date. `verify` names who is
still calling it, which inside one repo is a grep and across twenty services is the
question nobody can answer.

**`"header"` (the Stripe scheme).** The route never changes. The caller pins a dated
version, the server keeps ONE implementation —the current one— and an adapter per version
that changed a shape. That is what lets a version from years ago stay alive: nobody
maintains N implementations, they maintain N small adapters.

```toml
[api]
versioning = "header"
header     = "X-Api-Version"
default    = "2026-09-01"      # what an unpinned caller gets
support_window_days = 365      # nothing dies before a year
lts_window_days     = 1095     # an LTS lives three

[[api.version]]
date   = "2026-01-15"
lts    = true
sunset = "2029-01-15"

[[api.version]]
date       = "2026-05-01"
deprecated = "2026-09-01"
sunset     = "2027-06-01"

[[api.version]]
date = "2026-09-01"

[methods.getOrder]
http = "GET /orders/{orderId}"          # the route carries no version
out  = { orderId = "uuid", status = "string", customer = "json" }

# Only what CHANGED gets declared. A version with no entry did not change this
# method, and the chain skips it.
[methods.getOrder.at."2026-05-01"]
out     = { orderId = "uuid", status = "string", customerId = "uuid" }
adapter = "downgradeTo202605"
```

What comes out of that:

| Projection | What it gets |
| --- | --- |
| The types | one interface per old shape, and the adapter's, typed step by step: each one receives what the one above it produced |
| The chain | `adaptGetOrder(version, answer, adapters)`, applied newest → oldest, and `versionedRoutes` keyed by route |
| The resolution | `resolveApiVersion(pinned)`: absent is the default, unknown is a `400 unknown_api_version` — guessing which version somebody meant is worse than saying no |
| The headers | `Vary` always, the resolved version echoed, and the retirement of the pinned version if it has one |
| `axon openapi --api-version <date>` | the document as of that version: the shapes it promised, not today's |
| `axon versions` | the maintenance cycle: stage, days left, and what changed at each step |

The field mapping is business logic and a person writes it. What axon generates is the
plumbing and the **obligation**: a version declared with a changed shape and no
implementation does not compile.

`verify` refutes: an LTS with no `sunset` ("long term" with no date is not a promise), a
version served for less than the declared window, an LTS that dies before the ordinary
version that follows it, versions out of order —the list is the order the adapters are
applied in—, a past sunset still declared, a shape that changed with no `adapter` (naming
the fields), a shape identical to the current one, and the two schemes at once.

## `[aggregate.<name>]` and `[view.<name>]`

```toml
[aggregate.account]
events  = ["account.opened@v1", "account.deposited@v1"]
machine = "account"       # optional
snapshot_every = 0

[view.balances]
on = ["account.opened@v1", "account.deposited@v1"]
table = "view_balances"   # `view_<name>` by default
max_staleness_ms = 3000
```

The state is the stream, and the view is built by applying it. The tables
—`<name>_event`, the view's own and its checkpoint— are required by `verify` against the
real migrations. See [Event sourcing](./patterns.md#event-sourcing).

## `[metrics.<name>]`

```toml
[metrics.gmv]
on     = ["order.placed@v1"]
kind   = "sum"              # count · sum · avg
field  = "total"            # the field as the CONTRACT names it
by     = ["total.currency"] # dimensions, on top of the bucket
window = "1d"               # 1h · 1d · 1w · 1mo
```

A view in the warehouse next to the funnels, in the dialect of each one. What makes it
worth declaring is what `verify` refutes: a metric over an event nobody emits, a sum over
something that is not a number, a dimension the event does not declare, and a dimension
that is a personal field. See [Warehouse and metrics](./analytics.md#declared-metrics).
