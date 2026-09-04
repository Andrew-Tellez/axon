// La logica de negocio. La maquina de estados la impone el codigo generado.
import { PaymentsService, paymentNext, paymentCan,
         type CapturePaymentIn, type CapturePaymentOut,
         type RefundPaymentIn, type RefundPaymentOut,
         type OrderPlacedV1, type Envelope, type PaymentState } from "./contracts.ts";
import { bus, conectar, esperarDb, inbox, outbox, relay, servir, suscribir } from "../runtime.ts";
import type pg from "pg";

class Payments extends PaymentsService {
  #db: pg.Pool;
  constructor(b: any, i: any, o: any, db: pg.Pool) {
    super(b, i, o);
    this.#db = db;
  }

  /** Consume order.placed@v1. La deduplicacion ya la hizo dispatch(). */
  async onOrderPlaced(e: Envelope<OrderPlacedV1>): Promise<void> {
    await this.capturePayment({ orderId: e.data.orderId, amount: e.data.total }, e);
  }

  async capturePayment(input: CapturePaymentIn, e: Envelope<unknown>): Promise<CapturePaymentOut> {
    const paymentId = crypto.randomUUID();
    const cliente = await this.#db.connect();
    try {
      await cliente.query("BEGIN");
      // paymentNext revienta si la transicion no esta declarada en el manifiesto
      const estado: PaymentState = paymentNext("pending", "capture");
      await cliente.query(
        `INSERT INTO payment (id, order_id, amount_cents, status) VALUES ($1,$2,$3,$4)`,
        [paymentId, input.orderId, input.amount.amount, estado],
      );
      // Misma transaccion que el cambio de estado: eso es el outbox.
      await this.emitPaymentCapturedV1({ paymentId, orderId: input.orderId, amount: input.amount }, e);
      await cliente.query("COMMIT");
    } catch (err) {
      await cliente.query("ROLLBACK");
      throw err;
    } finally {
      cliente.release();
    }
    return { paymentId };
  }

  async refundPayment(input: RefundPaymentIn): Promise<RefundPaymentOut> {
    const { rows } = await this.#db.query(`SELECT status FROM payment WHERE id = $1`, [input.paymentId]);
    const actual = rows[0]?.status as PaymentState | undefined;
    if (!actual) throw new Error(`pago ${input.paymentId} no existe`);
    if (!paymentCan(actual, "refund")) throw new Error(`no se puede reembolsar desde ${actual}`);
    const estado = paymentNext(actual, "refund");
    await this.#db.query(`UPDATE payment SET status = $2 WHERE id = $1`, [input.paymentId, estado]);
    return { paymentId: input.paymentId, status: estado };
  }
}

const db = await esperarDb();
const nc = await conectar();
const b = bus(nc);
const svc = new Payments(b, inbox(db), outbox(db), db);

// El outbox no publica: publica el relay.
relay(db, b);

// dispatch() es el punto de entrada unico que genero axon: rutea y deduplica.
await suscribir(nc, ["order.placed@v1"], (e) => svc.dispatch(e));

servir(Number(process.env.PORT ?? 8080), {
  "POST /v1/payments": (body, e) => svc.capturePayment(body, e),
});
