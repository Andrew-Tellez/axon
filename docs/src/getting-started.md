# Your first manifest

```sh
axon init billing      # the whole layout, and it verifies clean
```

What follows explains every piece of what that writes. If you would rather read than
scaffold, keep going: the file it produces is the one below.


Ten minutes, without writing any business code yet. At the end you will have a verified
service, its infrastructure and its diagrams — all derived from one file.

## 1. Declare the service

```toml
# manifests/orders.toml
service = "orders"
version = "0.1.0"
owner   = "your-team"      # with no owner it does not get deployed
tier    = "2"              # criticality: decides SLO, alerts and sampling

[emits."order.placed@v1"]  # domain.happened@version, always
orderId    = "uuid"
customerId = "uuid"
total      = "money"       # a float for money is a bug waiting its turn

[methods.placeOrder]
http       = "POST /v1/orders"
auth       = "public"      # mandatory: the edge fails closed
rate_limit = 60            # mandatory if it is public
timeout_ms = 5000
idempotent = true          # mandatory if it mutates
in  = { customerId = "uuid", total = "money" }
out = { orderId = "uuid" }
```

## 2. Verify before writing a single line

```sh
axon verify manifests/
```

This will already tell you something. If you left out `auth`, if the route carries no
version, if the method mutates without being idempotent. **The point is for it to fail
here and not in production.**

## 3. Generate the contract

```sh
axon build manifests/orders.toml manifests/ --lang ts > src/contracts.ts
```

Out come the types, the envelope with the causal chain, the abstract base class and —if
there are `[[depends]]`— the clients with their retry policy. What does **not** come out
is your logic: you inherit from the class and write the abstract methods.

## 4. Bring the system up on your machine

```sh
axon infra manifests/ --target local > axon.local.yml
docker compose -f axon.local.yml up -d --wait
```

That brings up the broker, one database per service, the migrations applied, the edge,
the object storage, the trace backend and flagd if you declared flags.

**Local is not a separate subsystem: it is another render of the same neutral plan.**
That is why local and production cannot diverge.

## 5. Look at what you declared

```sh
axon graph   manifests/    # event topology
axon classes manifests/    # class diagram
axon seq     order.placed@v1 manifests/   # the expected causal flow
axon cap     manifests/    # what the CAP side you picked implies
```

They all emit Mermaid, which GitHub renders inside a ` ```mermaid ` block.

## 6. Record the contracts

When you publish:

```sh
axon baseline manifests/ > manifests/axon.baseline.json
```

From then on, `verify` blocks any incompatible change to an already published version.
[A published version is immutable](./verification.md).

## And now, the code

The only thing you write by hand:

```ts
import { OrdersService, type PlaceOrderIn, type PlaceOrderOut } from "./contracts.ts";

export class Orders extends OrdersService {
  async placeOrder(input: PlaceOrderIn, e: Envelope<unknown>): Promise<PlaceOrderOut> {
    const orderId = crypto.randomUUID();
    await this.db.insert(orderId, input);
    // `e` is the cause: the generated emitter propagates traceparent and correlationId
    await this.emitOrderPlacedV1({ orderId, ...input }, e);
    return { orderId };
  }
}
```

The complete, runnable example —three services, Postgres, NATS, a real outbox,
OpenTelemetry— is in
[`examples/`](https://github.com/Andrew-Tellez/axon/tree/main/examples), and
`./demo.sh` brings it up and checks that the real trace matches the manifest.
