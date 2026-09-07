// The business logic. The only thing a person writes.
import { OrdersService, fail, problem, httpRoutes, manifest, type PlaceOrderIn, type PlaceOrderOut,
         type GetOrderIn, type GetOrderOut, type Envelope } from "./contracts.ts";
import { startTelemetry } from "../telemetry.ts";
import { NotFound, bus, connectBroker, serve, waitForDb } from "../runtime.ts";
import type pg from "pg";

class Orders extends OrdersService {
  #db: pg.Pool;
  constructor(b: any, db: pg.Pool) {
    // no inbox: `orders` emits and consumes nothing, and the generated code
    // only asks for what the manifest declares
    super(b);
    this.#db = db;
  }

  /** One transaction with the role and the tenant set, and both die at the
   *  COMMIT. Both are needed: without `ROLE` the policy does not apply —the
   *  table's owner is a superuser and skips it— and without the tenant there is
   *  no policy to apply. And it has to be the SAME pool client: with
   *  `pool.query` each statement can go out over a different connection. */
  async #asTenant<T>(tenantId: string, fn: (c: pg.PoolClient) => Promise<T>): Promise<T> {
    const c = await this.#db.connect();
    try {
      await c.query("BEGIN");
      await c.query("SET LOCAL ROLE axon_app");
      // literal, not `set_config($1)`: a pooler intercepts the SET and may not
      // intercept the function with a bound parameter
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
    // The declared failure, thrown with `fail`: the code is checked against the
    // manifest, so this cannot drift from what the OpenAPI promises.
    if (input.total.amount <= 0) {
      fail("placeOrder", "order_rejected", "the total has to be positive");
    }
    const orderId = crypto.randomUUID();
    await this.#asTenant(input.tenantId, (c) =>
      c.query(
        `INSERT INTO "order" (id, tenant_id, customer_id, customer_email, total_cents, status)
         VALUES ($1,$2,$3,$4,$5,'placed')`,
        [orderId, input.tenantId, input.customerId, input.customerEmail, input.total.amount],
      ),
    );
    // `e` is the cause: the generated emitter propagates traceparent and correlationId.
    await this.emitOrderPlacedV1(
      {
        orderId,
        customerId: input.customerId,
        // declared `pii`: redacted in logs with `redact()`, excluded or hashed
        // in the warehouse, and masked in the analytics view
        customerEmail: input.customerEmail,
        total: input.total,
      },
      e,
    );
    return { orderId };
  }

  async getOrder(input: GetOrderIn): Promise<GetOrderOut> {
    // `tenant_id` in the WHERE is not redundant with the RLS: it is what tells
    // the sharder which node to go to. Without it, the query is not answered
    // wrong, it is rejected.
    const { rows } = await this.#asTenant(input.tenantId, (c) =>
      c.query(`SELECT * FROM "order" WHERE tenant_id = $1 AND id = $2`, [
        input.tenantId,
        input.orderId,
      ]),
    );
    if (!rows[0]) throw new NotFound(`order ${input.orderId} does not exist`);
    return {
      orderId: rows[0].id,
      status: rows[0].status,
      total: { amount: Number(rows[0].total_cents), currency: "MXN" },
    };
  }
}

startTelemetry();
const db = await waitForDb();
const svc = new Orders(bus(await connectBroker()), db);
serve(
  Number(process.env.PORT ?? 8080),
  {
    // Discovery: the service publishes its own manifest at the path the
    // generated contract names. `axon discover <url>` merges that with what is
    // on disk, so a registry can be built from what is RUNNING and not from what
    // somebody remembered to commit.
    "GET /.well-known/axon.json": async () => manifest,
    "POST /v1/tenants/{tenantId}/orders": (body, e, params) =>
      svc.placeOrder({ ...body, tenantId: params.tenantId }, e),
    "GET /v1/tenants/{tenantId}/orders/{orderId}": (_body, _e, params) =>
      svc.getOrder({ tenantId: params.tenantId, orderId: params.orderId }),
  },
  // startup fails if the manifest declares a route with no handler
  httpRoutes,
  // the declared failures, projected as problem+json
  problem,
);
