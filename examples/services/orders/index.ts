// La logica de negocio. Lo unico que escribe una persona.
import { OrdersService, type PlaceOrderIn, type PlaceOrderOut,
         type GetOrderIn, type GetOrderOut, type Envelope } from "./contracts.ts";
import { arrancarTelemetria } from "../telemetria.ts";
import { bus, conectar, esperarDb, inbox, servir } from "../runtime.ts";
import type pg from "pg";

class Orders extends OrdersService {
  #db: pg.Pool;
  constructor(b: any, i: any, db: pg.Pool) {
    super(b, i);
    this.#db = db;
  }

  async placeOrder(input: PlaceOrderIn, e: Envelope<unknown>): Promise<PlaceOrderOut> {
    const orderId = crypto.randomUUID();
    await this.#db.query(
      `INSERT INTO "order" (id, customer_id, total_cents, status) VALUES ($1,$2,$3,'placed')`,
      [orderId, input.customerId, input.total.amount],
    );
    // `e` es la causa: el emisor generado propaga traceparent y correlationId.
    await this.emitOrderPlacedV1(
      { orderId, customerId: input.customerId, total: input.total },
      e,
    );
    return { orderId };
  }

  async getOrder(input: GetOrderIn): Promise<GetOrderOut> {
    const { rows } = await this.#db.query(`SELECT * FROM "order" WHERE id = $1`, [input.orderId]);
    if (!rows[0]) throw new Error(`orden ${input.orderId} no existe`);
    return {
      orderId: rows[0].id,
      status: rows[0].status,
      total: { amount: Number(rows[0].total_cents), currency: "MXN" },
    };
  }
}

arrancarTelemetria();
const db = await esperarDb();
const svc = new Orders(bus(await conectar()), inbox(db), db);
servir(Number(process.env.PORT ?? 8080), {
  "POST /v1/orders": (body, e) => svc.placeOrder(body, e),
});
