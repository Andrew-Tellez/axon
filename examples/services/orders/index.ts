// La logica de negocio. Lo unico que escribe una persona.
import { OrdersService, httpRoutes, type PlaceOrderIn, type PlaceOrderOut,
         type GetOrderIn, type GetOrderOut, type Envelope } from "./contracts.ts";
import { arrancarTelemetria } from "../telemetria.ts";
import { NoEncontrado, bus, conectar, esperarDb, servir } from "../runtime.ts";
import type pg from "pg";

class Orders extends OrdersService {
  #db: pg.Pool;
  constructor(b: any, db: pg.Pool) {
    // sin inbox: `orders` emite y no consume nada, y el generado solo pide lo
    // que el manifiesto declara
    super(b);
    this.#db = db;
  }

  /** Una transaccion con el rol y el inquilino puestos, y los dos mueren en el
   *  COMMIT. Los dos hacen falta: sin `ROLE` la politica no aplica —el dueno
   *  de la tabla es superusuario y se la salta— y sin el inquilino no hay
   *  politica que aplicar. Y tiene que ser el MISMO cliente del pool: con
   *  `pool.query` cada sentencia puede salir por otra conexion. */
  async #comoInquilino<T>(tenantId: string, fn: (c: pg.PoolClient) => Promise<T>): Promise<T> {
    const c = await this.#db.connect();
    try {
      await c.query("BEGIN");
      await c.query("SET LOCAL ROLE axon_app");
      // literal, no `set_config($1)`: un pooler intercepta el SET y puede no
      // interceptar la funcion con parametro bindeado
      await c.query(`SET LOCAL axon.tenant = '${tenantId}'`);
      const r = await fn(c);
      await c.query("COMMIT");
      return r;
    } catch (err) {
      await c.query("ROLLBACK").catch(() => {});
      throw err;
    } finally {
      c.release();
    }
  }

  async placeOrder(input: PlaceOrderIn, e: Envelope<unknown>): Promise<PlaceOrderOut> {
    const orderId = crypto.randomUUID();
    await this.#comoInquilino(input.tenantId, (c) =>
      c.query(
        `INSERT INTO "order" (id, tenant_id, customer_id, customer_email, total_cents, status)
         VALUES ($1,$2,$3,$4,$5,'placed')`,
        [orderId, input.tenantId, input.customerId, input.customerEmail, input.total.amount],
      ),
    );
    // `e` es la causa: el emisor generado propaga traceparent y correlationId.
    await this.emitOrderPlacedV1(
      {
        orderId,
        customerId: input.customerId,
        // declarado `pii`: se redacta en logs con `redact()`, se excluye o
        // hashea en la bodega, y se enmascara en la vista de analitica
        customerEmail: input.customerEmail,
        total: input.total,
      },
      e,
    );
    return { orderId };
  }

  async getOrder(input: GetOrderIn): Promise<GetOrderOut> {
    // `tenant_id` en el WHERE no es redundante con la RLS: es lo que le dice
    // al sharder a que nodo ir. Sin el, la consulta no se responde mal, se
    // rechaza.
    const { rows } = await this.#comoInquilino(input.tenantId, (c) =>
      c.query(`SELECT * FROM "order" WHERE tenant_id = $1 AND id = $2`, [
        input.tenantId,
        input.orderId,
      ]),
    );
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
const svc = new Orders(bus(await conectar()), db);
servir(
  Number(process.env.PORT ?? 8080),
  {
    "POST /v1/tenants/{tenantId}/orders": (body, e, params) =>
      svc.placeOrder({ ...body, tenantId: params.tenantId }, e),
    "GET /v1/tenants/{tenantId}/orders/{orderId}": (_body, _e, params) =>
      svc.getOrder({ tenantId: params.tenantId, orderId: params.orderId }),
  },
  // el arranque falla si el manifiesto declara una ruta sin handler
  httpRoutes,
);
