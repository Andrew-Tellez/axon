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
