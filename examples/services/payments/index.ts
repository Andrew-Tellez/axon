// La logica de negocio. La maquina de estados la impone el codigo generado.
import { PaymentsService, rutasHttp, paymentNext, paymentCan, flagCobroV2, flagCortarStripe,
         type CapturePaymentIn, type CapturePaymentOut,
         type RefundPaymentIn, type RefundPaymentOut,
         type PayoutMerchantIn, type PayoutMerchantOut,
         type OrderPlacedV1, type Envelope, type PaymentState } from "./contracts.ts";
import { arrancarTelemetria } from "../telemetria.ts";
import { arrancarFlags, flags } from "../flags.ts";
import { bus, conectar, esperarDb, inbox, outbox, relay, servir, suscribir } from "../runtime.ts";
import type pg from "pg";

export class Payments extends PaymentsService {
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
    // El accesor generado exige el campo por el que se fija: no se puede
    // evaluar este flag por peticion aunque uno quiera.
    const inquilino = process.env.AXON_TENANT ?? "inquilino-demo";
    const cobroNuevo = await flagCobroV2(flags, inquilino);
    if (await flagCortarStripe(flags)) {
      throw new Error("cobro cortado por el interruptor de emergencia");
    }
    const paymentId = crypto.randomUUID();
    const cliente = await this.#db.connect();
    try {
      await cliente.query("BEGIN");
      // paymentNext revienta si la transicion no esta declarada en el manifiesto
      // Las dos ramas del rollout terminan en el mismo estado declarado: el
      // flag cambia el camino, no la maquina de estados.
      const estado: PaymentState = paymentNext("pending", "capture");
      if (cobroNuevo) {
        // camino nuevo, detras del rollout del 10%
      }
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

  /** El tope del comercio. Arriba de esto el pago se rechaza, y ese rechazo
   *  llega DESPUES de haber cobrado: es el fallo que la saga tiene que
   *  compensar. */
  static readonly TOPE_COMERCIO = 100_000;

  /** Registra el intento. Sin esto, la politica de reintentos que declara el
   *  manifiesto no se puede comprobar: no hay forma de saber cuantas veces
   *  llego la llamada. */
  async #intento(metodo: string, paymentId: string): Promise<number> {
    const { rows } = await this.#db.query(
      `INSERT INTO intento (id, metodo, payment_id) VALUES (gen_random_uuid(), $1, $2)
       RETURNING (SELECT count(*) FROM intento WHERE metodo = $1 AND payment_id = $2) AS n`,
      [metodo, paymentId],
    );
    return Number(rows[0].n) + 1;
  }

  async payoutMerchant(input: PayoutMerchantIn): Promise<PayoutMerchantOut> {
    await this.#intento("payout", input.paymentId);
    // Interruptor del demo, no del negocio: hace que la llamada exceda su
    // propio timeout para poder MEDIR los reintentos declarados. Sin esto no
    // hay fallo transitorio que contar.
    const lento = Number(process.env.AXON_DEMO_PAYOUT_LENTO_MS ?? 0);
    if (lento > 0) await new Promise((r) => setTimeout(r, lento));
    if (input.amount.amount > Payments.TOPE_COMERCIO) {
      throw new Error(`monto ${input.amount.amount} supera el tope del comercio`);
    }
    const payoutId = crypto.randomUUID();
    // `idempotent = true` en el manifiesto no es una etiqueta: reintentar tiene
    // que no pagar dos veces, y eso lo sostiene el UNIQUE sobre payment_id
    const { rows } = await this.#db.query(
      `INSERT INTO payout (id, payment_id, cents) VALUES ($1,$2,$3)
       ON CONFLICT (payment_id) DO UPDATE SET cents = payout.cents
       RETURNING id`,
      [payoutId, input.paymentId, input.amount.amount],
    );
    return { payoutId: rows[0].id };
  }

  /** Compensacion del cobro. Tiene que tolerar que no haya nada que deshacer:
   *  el coordinador la llama tambien cuando el cobro quedo en duda —un timeout
   *  no dice que del otro lado no paso nada— y cuando ya se reembolso, porque
   *  se reintenta hasta que entra. */
  async refundPayment(input: RefundPaymentIn): Promise<RefundPaymentOut> {
    const n = await this.#intento("refund", input.paymentId);
    // El otro interruptor del demo: falla las primeras N veces y despues
    // entra. Es lo que permite medir que los reintentos de la COMPENSACION son
    // lo que salva a la saga de quedarse atascada.
    const fallar = Number(process.env.AXON_DEMO_REFUND_FALLAR_VECES ?? 0);
    if (n <= fallar) throw new Error(`reembolso rechazado (intento ${n} de ${fallar})`);
    const { rows } = await this.#db.query(`SELECT status FROM payment WHERE id = $1`, [input.paymentId]);
    const actual = rows[0]?.status as PaymentState | undefined;
    if (!actual) return { paymentId: input.paymentId, status: "sin_cobro" };
    if (actual === "refunded") return { paymentId: input.paymentId, status: actual };
    if (!paymentCan(actual, "refund")) throw new Error(`no se puede reembolsar desde ${actual}`);
    const estado = paymentNext(actual, "refund");
    await this.#db.query(`UPDATE payment SET status = $2 WHERE id = $1`, [input.paymentId, estado]);
    return { paymentId: input.paymentId, status: estado };
  }
}

// Arranque solo cuando se ejecuta como programa, no al importarlo desde un test.
if (process.env.NODE_TEST_CONTEXT === undefined) await main();

async function main() {
arrancarTelemetria();
await arrancarFlags();
const db = await esperarDb();
const nc = await conectar();
const b = bus(nc);
const svc = new Payments(b, inbox(db), outbox(db), db);

// El outbox no publica: publica el relay.
relay(db, b);

// dispatch() es el punto de entrada unico que genero axon: rutea y deduplica.
await suscribir(nc, ["order.placed@v1"], (e) => svc.dispatch(e));

servir(
  Number(process.env.PORT ?? 8080),
  {
    "POST /v1/payments": (body, e) => svc.capturePayment(body, e),
    "POST /v1/payments/{paymentId}/refunds": (_b, _e, params) =>
      svc.refundPayment({ paymentId: params.paymentId }),
    "POST /v1/payouts": (body) => svc.payoutMerchant(body),
  },
  rutasHttp,
);
}
