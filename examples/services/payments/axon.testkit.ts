// generado por axon — no editar.
//
// Wire it up from your own test file:
//
//   import { contractTests, machineTests } from "./axon.testkit.ts";
//   import { Payments } from "./index.ts";
//   contractTests((bus, inbox, outbox) => new Payments(bus, inbox, outbox));
//   machineTests();
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  newEnvelope,
  type Envelope,
  type Bus,
  type Inbox,
  PaymentsService,
  type Outbox,
  type OrderPlacedV1,
  paymentTransitions,
  paymentNext,
  paymentCan,
  type PaymentState,
  type PaymentAction,
} from "./contracts.ts";

// In-memory doubles. Deterministic and dependency-free: contract tests
// need no infrastructure, integration tests do.
export class FakeBus implements Bus {
  readonly published: Envelope<unknown>[] = [];
  async publish(e: Envelope<unknown>) {
    this.published.push(e);
  }
}

export class MemoryInbox implements Inbox {
  readonly seen = new Set<string>();
  async once(id: string, fn: () => Promise<void>) {
    if (this.seen.has(id)) return;
    this.seen.add(id);
    await fn();
  }
}

export class FakeOutbox implements Outbox<unknown> {
  readonly staged: Envelope<unknown>[] = [];
  async stage(e: Envelope<unknown>, _tx: unknown) {
    this.staged.push(e);
  }
}

// Fixtures derived from the schema declared by each event's OWNER, not
// from what the consumer believes it receives: that is where drift shows up.
export const fixtureOrderPlacedV1: OrderPlacedV1 = {
  orderId: "00000000-0000-4000-8000-000000000000",
  customerId: "00000000-0000-4000-8000-000000000000",
  customerEmail: "customerEmail",
  total: { amount: 100, currency: "MXN" },
};

/** Contract tests. `make` returns your implementation of the service. */
export function contractTests(make: (bus: FakeBus, inbox: MemoryInbox, outbox: FakeOutbox) => PaymentsService) {
  const setUp = () => {
    const bus = new FakeBus();
    const inbox = new MemoryInbox();
    const outbox = new FakeOutbox();
    const svc = make(bus, inbox, outbox);
    return { svc, bus, inbox, outbox };
  };

  describe("payments · contract", () => {
    it("accepts order.placed@v1 exactly as its owner emits it", async () => {
      const { svc } = setUp();
      await svc.dispatch(newEnvelope("order.placed@v1", "test", fixtureOrderPlacedV1));
    });

    it("a second delivery of order.placed@v1 does not repeat the effect", async () => {
      const { svc, outbox } = setUp();
      const e = newEnvelope("order.placed@v1", "test", fixtureOrderPlacedV1);
      await svc.dispatch(e);
      const after = outbox.staged.length;
      await svc.dispatch(e);
      assert.equal(outbox.staged.length, after, "the same envelope took effect twice");
    });
    it("propagates the causal chain when reacting to order.placed@v1", async () => {
      const { svc, outbox } = setUp();
      const cause = newEnvelope("order.placed@v1", "test", fixtureOrderPlacedV1);
      await svc.dispatch(cause);
      const out = outbox.staged;
      assert.ok(out.length > 0, "it emitted nothing");
      for (const e of out) {
        assert.equal(e.causationId, cause.id, "causationId does not point at the cause");
        assert.equal(e.correlationId, cause.correlationId, "the flow was lost");
        assert.equal(e.traceparent.split("-")[1], cause.traceparent.split("-")[1], "the trace was lost");
      }
    });
    it("nothing gets published outside the outbox", async () => {
      const { bus } = setUp();
      assert.equal(bus.published.length, 0, "dual-write: the handler touched the bus");
    });
  });
}

/** State machine tests. They need none of your code. */
export function machineTests() {
  describe("payments · machine payment", () => {
    it("every declared transition is legal from its source states", () => {
      for (const [action, t] of Object.entries(paymentTransitions)) {
        for (const from of t.from) {
          assert.equal(paymentNext(from, action as PaymentAction), t.to);
          assert.ok(paymentCan(from, action as PaymentAction));
        }
      }
    });

    it("an undeclared transition blows up", () => {
      const states: PaymentState[] = ["pending", "captured", "failed", "refunded"];
      for (const [action, t] of Object.entries(paymentTransitions)) {
        for (const e of states.filter((s) => !t.from.includes(s))) {
          assert.throws(() => paymentNext(e, action as PaymentAction));
          assert.equal(paymentCan(e, action as PaymentAction), false);
        }
      }
    });
  });
}

