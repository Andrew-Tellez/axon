// La logica de negocio. Lo unico que escribe una persona.
import { OrdersService, rutasHttp, type PlaceOrderIn, type PlaceOrderOut,
         type GetOrderIn, type GetOrderOut, type Envelope } from "./contracts.ts";
import { arrancarTelemetria } from "../telemetria.ts";
import { NoEncontrado, bus, conectar, esperarDb, inbox, servir } from "../runtime.ts";
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
      `INSERT INTO "order" (id, customer_id, customer_email, total_cents, status)
       VALUES ($1,$2,$3,$4,'placed')`,
      [orderId, input.customerId, input.customerEmail, input.total.amount],
    );
    // `e` es la causa: el emisor generado propaga traceparent y correlationId.
    await this.emitOrderPlacedV1(
      {
        orderId,
        customerId: input.customerId,
        // declarado `pii`: se redacta en logs con `redactar()`, se excluye o
        // hashea en la bodega, y se enmascara en la vista de analitica
        customerEmail: input.customerEmail,
        total: input.total,
      },
      e,
    );
    return { orderId };
  }

  async getOrder(input: GetOrderIn): Promise<GetOrderOut> {
    const { rows } = await this.#db.query(`SELECT * FROM "order" WHERE id = $1`, [input.orderId]);
    if (!rows[0]) throw new NoEncontrado(`orden ${input.orderId} no existe`);
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
servir(
  Number(process.env.PORT ?? 8080),
  {
    "POST /v1/orders": (body, e) => svc.placeOrder(body, e),
    "GET /v1/orders/{orderId}": (_body, _e, params) => svc.getOrder({ orderId: params.orderId }),
  },
  // el arranque falla si el manifiesto declara una ruta sin handler
  rutasHttp,
);
