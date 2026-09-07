// The adapters axon does NOT generate, on purpose: the framework keeps what
// crosses processes, and the glue to the infrastructure belongs to whoever
// deploys. This is all it takes. ~120 lines for the three services.
import { connect, type NatsConnection, StringCodec } from "nats";
import pg from "pg";
import { appendFile } from "node:fs/promises";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
// telemetry.ts imports the Envelope type from here; that import is types only,
// so it is erased at compile time and there is no cycle at runtime.
import { annotate, atEdge, inProducer, inSpan } from "./telemetry.ts";
// axon's contract. The generated code emits these same four shapes in every
// service; structural typing makes them fit without importing each other.
// (Once an @axon/runtime package exists, they will live there and the generated
// code will import them.)
export interface Envelope<T> {
  id: string; type: string; source: string; time: string;
  traceparent: string; correlationId: string; causationId: string | null; data: T;
}
export interface Bus { publish(e: Envelope<unknown>): Promise<void>; }
// `tx` mandatory: it is what makes it impossible to write the event outside the
// transaction that changes the state. See `outbox()` below.
export interface Outbox<Tx = unknown> { stage(e: Envelope<unknown>, tx: Tx): Promise<void>; }
export interface Inbox { once(id: string, fn: () => Promise<void>): Promise<void>; }

const sc = StringCodec();
const TRACE = process.env.AXON_TRACE_LOG;

/** An NDJSON log of envelopes is all `axon trace` needs. */
async function trace(e: Envelope<unknown>) {
  if (!TRACE) return;
  await appendFile(TRACE, JSON.stringify(e) + "\n").catch(() => {});
}

// An unhandled rejection kills the process in silence. Let it leave a trace.
process.on("unhandledRejection", (r) =>
  console.error(`[${process.env.AXON_SERVICE}] unhandled rejection:`, r),
);
process.on("uncaughtException", (e) =>
  console.error(`[${process.env.AXON_SERVICE}] uncaught exception:`, e),
);

export async function connectBroker(): Promise<NatsConnection> {
  const servers = process.env.AXON_BROKER_URL ?? "nats://localhost:4222";
  for (let i = 0; ; i++) {
    try {
      return await connect({ servers });
    } catch (err) {
      if (i >= 20) throw err;
      await new Promise((r) => setTimeout(r, 500));
    }
  }
}

/** The event's name carries `@`, which NATS does not allow in a subject. */
export const subject = (type: string) => type.replace("@", ".");

export function bus(nc: NatsConnection): Bus {
  return {
    async publish(e) {
      // the span goes first: it rewrites the traceparent, and only then is the
      // message serialised and written to the log
      await inProducer(`publish ${e.type}`, e, { "messaging.operation": "publish" }, async () => {
        await trace(e);
        nc.publish(subject(e.type), sc.encode(JSON.stringify(e)));
      });
    },
  };
}

export async function subscribe(
  nc: NatsConnection,
  types: string[],
  handler: (e: Envelope<unknown>) => Promise<void>,
) {
  for (const t of types) {
    const sub = nc.subscribe(subject(t), { queue: process.env.AXON_SERVICE });
    (async () => {
      for await (const msg of sub) {
        const e = JSON.parse(sc.decode(msg.data)) as Envelope<unknown>;
        try {
          await inSpan(
            `process ${e.type}`,
            e,
            { "messaging.operation": "process", "messaging.source.name": subject(e.type) },
            () => handler(e),
          );
        } catch (err) {
          // In production the broker's DLQ does this; locally, visible noise.
          console.error(`[${process.env.AXON_SERVICE}] ${e.type} failed:`, err);
        }
      }
    })();
  }
}

/** Idempotent inbox: the PK's uniqueness is the deduplication. */
export function inbox(db: pg.Pool): Inbox {
  return {
    async once(id, fn) {
      const r = await db.query("INSERT INTO inbox_seen (id) VALUES ($1) ON CONFLICT DO NOTHING", [id]);
      if (r.rowCount === 0) return; // already processed
      await fn();
    },
  };
}

/** The outbox writes into the caller's transaction, not into one of its own.
 *
 *  It used to take the pool and `stage` opened its own connection: the event
 *  committed by itself, so a rolled-back transaction left the event with no row
 *  and the relay published something that never happened. Measured, not assumed:
 *  0 payments and 1 event in the outbox. */
export function outbox(): Outbox<pg.PoolClient> {
  return {
    async stage(e, tx) {
      // the outbox stores the producer's traceparent, so what the relay
      // publishes still hangs off whoever generated it
      await inProducer(`stage ${e.type}`, e, { "messaging.operation": "create" }, () =>
        save(tx, e),
      );
    },
  };
}

async function save(db: pg.Pool | pg.PoolClient, e: Envelope<unknown>) {
  {
      await db.query(
        `INSERT INTO outbox (id, type, source, time, traceparent, correlation_id, causation_id, data)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)`,
        [e.id, e.type, e.source, e.time, e.traceparent, e.correlationId, e.causationId, JSON.stringify(e.data)],
      );
  }
}

/** The relay: the only thing that really publishes when there is an outbox. */
export function relay(db: pg.Pool, b: Bus, ms = 200) {
  const tick = async () => {
    const { rows } = await db.query(
      "SELECT * FROM outbox WHERE published_at IS NULL ORDER BY time LIMIT 20",
    );
    for (const r of rows) {
      await b.publish({
        id: r.id, type: r.type, source: r.source, time: r.time,
        traceparent: r.traceparent, correlationId: r.correlation_id,
        causationId: r.causation_id, data: r.data,
      });
      await db.query("UPDATE outbox SET published_at = now() WHERE id = $1", [r.id]);
    }
  };
  setInterval(() => void tick().catch((e) => console.error("relay:", e)), ms);
}

export async function waitForDb(): Promise<pg.Pool> {
  const pool = new pg.Pool({ connectionString: process.env.DATABASE_URL });
  for (let i = 0; ; i++) {
    try {
      await pool.query("SELECT 1");
      return pool;
    } catch (err) {
      if (i >= 30) throw err;
      await new Promise((r) => setTimeout(r, 500));
    }
  }
}

type Route = (body: any, e: Envelope<unknown>, params: Record<string, string>) => Promise<unknown>;

/** A resource that does not exist is the client's error, not the server's. */
export class NotFound extends Error {}

/** `POST /v1/orders/{orderId}` -> a matcher that captures `orderId`. */
function compile(key: string) {
  const [method, pattern] = key.split(" ");
  const names: string[] = [];
  const regex = new RegExp(
    "^" +
      pattern
        .split("/")
        .map((seg) => {
          if (!seg.startsWith("{")) return seg.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
          names.push(seg.slice(1, -1));
          return "([^/]+)";
        })
        .join("/") +
      "$",
  );
  return { method, regex, names };
}

/** A minimal server. The real HTTP framework is each team's choice.
 *
 *  `declared` are the manifest's routes: if any of them has no handler, the
 *  process does not start. Without this, a route declared and not served returns
 *  a 404 in production and shows up in no test. */
export function serve(
  port: number,
  routes: Record<string, Route>,
  declared: readonly string[] = [],
  // The generated `problem`: the manifest's failures projected as RFC 7807.
  // It is passed in and not imported because the runtime is shared and each
  // service's codes are its own.
  toProblem?: (err: unknown, e?: Envelope<unknown>) => { status: number },
) {
  const missing = declared.filter((d) => !(d in routes));
  if (missing.length) {
    throw new Error(
      `${process.env.AXON_SERVICE}: the manifest declares routes with no handler: ${missing.join(", ")}`,
    );
  }
  const table = Object.entries(routes).map(([key, fn]) => ({ ...compile(key), key, fn }));

  createServer(async (req: IncomingMessage, res: ServerResponse) => {
    if (req.url === "/healthz") return res.writeHead(200).end("ok");
    const path = (req.url ?? "/").split("?")[0];
    const hit = table
      .filter((r) => r.method === req.method)
      .map((r) => ({ r, m: r.regex.exec(path) }))
      .find(({ m }) => m);
    if (!hit) {
      // RFC 7807, the same format the generated OpenAPI declares
      res.writeHead(404, { "content-type": "application/problem+json" });
      return res.end(JSON.stringify({ type: "about:blank", title: "not found", status: 404 }));
    }
    const { r, m } = hit;
    const params = Object.fromEntries(r.names.map((n, i) => [n, decodeURIComponent(m![i + 1])]));

    const chunks: Buffer[] = [];
    for await (const c of req) chunks.push(c as Buffer);
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : {};

    // OTel opens the trace and the envelope inherits it, not the other way
    // round: if the envelope invented the traceparent, the root span would hang
    // off a parent that never existed. If the caller already sends one, it continues.
    const incoming = req.headers["traceparent"];
    await atEdge(
      r.key,
      typeof incoming === "string" ? incoming : undefined,
      { "http.request.method": req.method ?? "", "http.route": r.key },
      async (traceparent: string) => {
        const root: Envelope<unknown> = {
          id: crypto.randomUUID(),
          type: r.key,
          source: "http",
          time: new Date().toISOString(),
          traceparent,
          correlationId:
            (typeof req.headers["x-correlation-id"] === "string"
              ? req.headers["x-correlation-id"]
              : undefined) ?? crypto.randomUUID(),
          causationId: null,
          data: body,
        };
        annotate({ "messaging.message.id": root.id, "axon.correlation_id": root.correlationId });
        await trace(root);
        try {
          const out = await r.fn(body, root, params);
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify(out));
        } catch (err) {
          // It is not rethrown: createServer's handler is async, so a throw here
          // comes out as an unhandled rejection and Node kills the process in the
          // middle of the response.
          // A DECLARED failure goes out with its code and its status: that is
          // the whole point of declaring it. Anything else is still a 500,
          // because a failure nobody declared is genuinely unexpected.
          const body = err instanceof NotFound
            ? { type: "about:blank", title: "not found", status: 404, traceId: traceparent.split("-")[1] }
            : toProblem?.(err, root) ?? {
                type: "about:blank",
                title: String(err),
                status: 500,
                traceId: traceparent.split("-")[1],
              };
          const status = body.status;
          if (status >= 500) {
            console.error(`[${process.env.AXON_SERVICE}] ${r.key} failed:`, err);
            annotate({ "error.type": String(err) });
          }
          res.writeHead(status, { "content-type": "application/problem+json" });
          res.end(JSON.stringify({ ...body, status }));
        }
      },
    );
  }).listen(port, () => console.log(`[${process.env.AXON_SERVICE}] listening on :${port}`));
}
