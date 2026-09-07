// The business logic. The state machine is enforced by the generated code.
import { PaymentsService, fail, problem, httpRoutes, manifest, paymentNext, paymentCan, flagChargeV2, flagStripeKill,
         type CapturePaymentIn, type CapturePaymentOut,
         type RefundPaymentIn, type RefundPaymentOut,
         type PayoutMerchantIn, type PayoutMerchantOut,
         type OrderPlacedV1, type Envelope, type PaymentState } from "./contracts.ts";
import { startTelemetry } from "../telemetry.ts";
import { startFlags, flags } from "../flags.ts";
import { bus, connectBroker, inbox, outbox, relay, serve, subscribe, waitForDb } from "../runtime.ts";
import type pg from "pg";

export class Payments extends PaymentsService {
  #db: pg.Pool;
  constructor(b: any, i: any, o: any, db: pg.Pool) {
    super(b, i, o);
    this.#db = db;
  }

  /** Consumes order.placed@v1. dispatch() already did the deduplication. */
  async onOrderPlaced(e: Envelope<OrderPlacedV1>): Promise<void> {
    await this.capturePayment({ orderId: e.data.orderId, amount: e.data.total }, e);
  }

  async capturePayment(input: CapturePaymentIn, e: Envelope<unknown>): Promise<CapturePaymentOut> {
    // The generated accessor requires the field it is pinned by: this flag
    // cannot be evaluated per request even if somebody wanted to.
    const tenant = process.env.AXON_TENANT ?? "demo-tenant";
    const newCharge = await flagChargeV2(flags, tenant);
    if (await flagStripeKill(flags)) {
      throw new Error("charging cut off by the emergency switch");
    }
    const paymentId = crypto.randomUUID();
    const client = await this.#db.connect();
    try {
      await client.query("BEGIN");
      // paymentNext blows up if the transition is not declared in the manifest.
      // Both branches of the rollout end in the same declared state: the flag
      // changes the path, not the state machine.
      const state: PaymentState = paymentNext("pending", "capture");
      if (newCharge) {
        // the new path, behind the 10% rollout
      }
      await client.query(
        `INSERT INTO payment (id, order_id, amount_cents, status) VALUES ($1,$2,$3,$4)`,
        [paymentId, input.orderId, input.amount.amount, state],
      );
      // The same transaction as the state change: that is the outbox.
      // `client` is the SAME transaction as the INSERT above: the parameter is
      // mandatory precisely so the event cannot be written outside it.
      await this.emitPaymentCapturedV1(
        { paymentId, orderId: input.orderId, amount: input.amount },
        client,
        e,
      );
      // A demo switch: it blows up AFTER staging the event and BEFORE the
      // COMMIT. It is there to measure whether the outbox is really
      // transactional: if it were not, the payment would not exist and the
      // event would.
      if (process.env.AXON_DEMO_BREAK_AFTER_STAGE === "1") {
        throw new Error("broken on purpose after the stage");
      }
      await client.query("COMMIT");
    } catch (err) {
      await client.query("ROLLBACK");
      throw err;
    } finally {
      client.release();
    }
    return { paymentId };
  }

  /** The merchant's ceiling. Above this the payout is rejected, and that
   *  rejection arrives AFTER the charge already went through: it is the failure
   *  the saga has to compensate. */
  static readonly MERCHANT_CEILING = 100_000;

  /** Records the attempt. Without this, the retry policy the manifest declares
   *  cannot be checked: there is no way to know how many times the call
   *  arrived. */
  async #attempt(method: string, paymentId: string): Promise<number> {
    const { rows } = await this.#db.query(
      `INSERT INTO attempt (id, method, payment_id) VALUES (gen_random_uuid(), $1, $2)
       RETURNING (SELECT count(*) FROM attempt WHERE method = $1 AND payment_id = $2) AS n`,
      [method, paymentId],
    );
    return Number(rows[0].n) + 1;
  }

  async payoutMerchant(input: PayoutMerchantIn): Promise<PayoutMerchantOut> {
    await this.#attempt("payout", input.paymentId);
    // A demo switch, not a business one: it makes the call exceed its own
    // timeout so the declared retries can be MEASURED. Without it there is no
    // transient failure to count.
    const slow = Number(process.env.AXON_DEMO_PAYOUT_SLOW_MS ?? 0);
    if (slow > 0) await new Promise((r) => setTimeout(r, slow));
    // A DECLARED failure, and `fail` is typed against the manifest: writing a
    // code that is not declared does not compile. The rail is retriable and the
    // ceiling is not, and the caller's client already knows which is which.
    if (process.env.AXON_DEMO_RAIL_BUSY === "1") fail("payoutMerchant", "rail_busy");
    if (input.amount.amount > Payments.MERCHANT_CEILING) {
      fail(
        "payoutMerchant",
        "merchant_ceiling",
        `${input.amount.amount} is over the ${Payments.MERCHANT_CEILING} ceiling`,
      );
    }
    const payoutId = crypto.randomUUID();
    // `idempotent = true` in the manifest is not a label: retrying has to not
    // pay twice, and what holds that up is the UNIQUE on payment_id
    const { rows } = await this.#db.query(
      `INSERT INTO payout (id, payment_id, cents) VALUES ($1,$2,$3)
       ON CONFLICT (payment_id) DO UPDATE SET cents = payout.cents
       RETURNING id`,
      [payoutId, input.paymentId, input.amount.amount],
    );
    return { payoutId: rows[0].id };
  }

  /** The charge's compensation. It has to tolerate there being nothing to undo:
   *  the coordinator also calls it when the charge was left in doubt —a timeout
   *  does not say nothing happened on the other side— and when it was already
   *  refunded, because it is retried until it gets through. */
  async refundPayment(input: RefundPaymentIn): Promise<RefundPaymentOut> {
    const n = await this.#attempt("refund", input.paymentId);
    // The other demo switch: it fails the first N times and then gets through.
    // It is what makes it possible to measure that the COMPENSATION's retries
    // are what saves the saga from ending up stuck.
    const failTimes = Number(process.env.AXON_DEMO_REFUND_FAIL_TIMES ?? 0);
    if (n <= failTimes) fail("refundPayment", "refund_rejected", `attempt ${n} of ${failTimes}`);
    const { rows } = await this.#db.query(`SELECT status FROM payment WHERE id = $1`, [input.paymentId]);
    const current = rows[0]?.status as PaymentState | undefined;
    if (!current) return { paymentId: input.paymentId, status: "no_charge" };
    if (current === "refunded") return { paymentId: input.paymentId, status: current };
    if (!paymentCan(current, "refund")) throw new Error(`cannot refund from ${current}`);
    const state = paymentNext(current, "refund");
    await this.#db.query(`UPDATE payment SET status = $2 WHERE id = $1`, [input.paymentId, state]);
    return { paymentId: input.paymentId, status: state };
  }
}

// Startup only when run as a program, not when imported from a test.
if (process.env.NODE_TEST_CONTEXT === undefined) await main();

async function main() {
startTelemetry();
await startFlags();
const db = await waitForDb();
const nc = await connectBroker();
const b = bus(nc);
const svc = new Payments(b, inbox(db), outbox(), db);

// The outbox does not publish: the relay does.
relay(db, b);

// dispatch() is the single entry point axon generated: it routes and deduplicates.
await subscribe(nc, ["order.placed@v1"], (e) => svc.dispatch(e));

serve(
  Number(process.env.PORT ?? 8080),
  {
    // Discovery: the service publishes its own manifest at the path the
    // generated contract names. `axon discover <url>` merges that with what is
    // on disk, so a registry can be built from what is RUNNING and not from what
    // somebody remembered to commit.
    "GET /.well-known/axon.json": async () => manifest,
    "POST /v1/payments": (body, e) => svc.capturePayment(body, e),
    "POST /v1/payments/{paymentId}/refunds": (_b, _e, params) =>
      svc.refundPayment({ paymentId: params.paymentId }),
    "POST /v1/payouts": (body) => svc.payoutMerchant(body),
  },
  httpRoutes,
  // the declared failures, projected as problem+json
  problem,
);
}
