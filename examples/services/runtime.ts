// Los adaptadores que axon NO genera, a proposito: el framework se queda con
// lo que cruza procesos, y el pegamento con la infra lo pone quien despliega.
// Esto es todo lo que hace falta. ~120 lineas para los dos servicios.
import { connect, type NatsConnection, StringCodec } from "nats";
import pg from "pg";
import { appendFile } from "node:fs/promises";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
// El contrato de axon. El codigo generado emite estas mismas cuatro formas en
// cada servicio; el tipado estructural hace que encajen sin importarse entre si.
// (Cuando exista un paquete @axon/runtime, viviran aqui y el generado importara.)
export interface Envelope<T> {
  id: string; type: string; source: string; time: string;
  traceparent: string; correlationId: string; causationId: string | null; data: T;
}
export interface Bus { publish(e: Envelope<unknown>): Promise<void>; }
export interface Outbox { stage(e: Envelope<unknown>): Promise<void>; }
export interface Inbox { once(id: string, fn: () => Promise<void>): Promise<void>; }

const sc = StringCodec();
const TRACE = process.env.AXON_TRACE_LOG;

/** Un log NDJSON de envelopes es todo lo que `axon trace` necesita. */
async function traza(e: Envelope<unknown>) {
  if (!TRACE) return;
  await appendFile(TRACE, JSON.stringify(e) + "\n").catch(() => {});
}

export async function conectar(): Promise<NatsConnection> {
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

/** El nombre del evento lleva `@`, que NATS no admite como subject. */
export const subject = (tipo: string) => tipo.replace("@", ".");

export function bus(nc: NatsConnection): Bus {
  return {
    async publish(e) {
      await traza(e);
      nc.publish(subject(e.type), sc.encode(JSON.stringify(e)));
    },
  };
}

export async function suscribir(
  nc: NatsConnection,
  tipos: string[],
  handler: (e: Envelope<unknown>) => Promise<void>,
) {
  for (const tipo of tipos) {
    const sub = nc.subscribe(subject(tipo), { queue: process.env.AXON_SERVICE });
    (async () => {
      for await (const msg of sub) {
        const e = JSON.parse(sc.decode(msg.data)) as Envelope<unknown>;
        try {
          await handler(e);
        } catch (err) {
          // En produccion esto lo hace la DLQ del broker; en local, ruido visible.
          console.error(`[${process.env.AXON_SERVICE}] ${e.type} fallo:`, err);
        }
      }
    })();
  }
}

/** Inbox idempotente: la unicidad del PK es la deduplicacion. */
export function inbox(db: pg.Pool): Inbox {
  return {
    async once(id, fn) {
      const r = await db.query("INSERT INTO inbox_seen (id) VALUES ($1) ON CONFLICT DO NOTHING", [id]);
      if (r.rowCount === 0) return; // ya procesado
      await fn();
    },
  };
}

/** Outbox: el evento entra en la misma transaccion que el cambio de estado. */
export function outbox(db: pg.Pool): Outbox {
  return {
    async stage(e) {
      await db.query(
        `INSERT INTO outbox (id, type, source, time, traceparent, correlation_id, causation_id, data)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)`,
        [e.id, e.type, e.source, e.time, e.traceparent, e.correlationId, e.causationId, JSON.stringify(e.data)],
      );
    },
  };
}

/** El relay: lo unico que publica de verdad cuando hay outbox. */
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

export async function esperarDb(): Promise<pg.Pool> {
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

type Ruta = (body: any, e: Envelope<unknown>) => Promise<unknown>;

/** Servidor minimo. El framework real de HTTP lo elige cada equipo. */
export function servir(port: number, rutas: Record<string, Ruta>) {
  createServer(async (req: IncomingMessage, res: ServerResponse) => {
    if (req.url === "/healthz") return res.writeHead(200).end("ok");
    const clave = `${req.method} ${req.url}`;
    const ruta = rutas[clave];
    if (!ruta) {
      // RFC 7807, el mismo formato que declara el OpenAPI generado
      res.writeHead(404, { "content-type": "application/problem+json" });
      return res.end(JSON.stringify({ type: "about:blank", title: "no encontrado", status: 404 }));
    }
    const chunks: Buffer[] = [];
    for await (const c of req) chunks.push(c as Buffer);
    const body = chunks.length ? JSON.parse(Buffer.concat(chunks).toString()) : {};
    // El envelope tambien nace en el borde HTTP: la traza empieza aqui.
    const raiz = { id: crypto.randomUUID(), type: clave, source: "http",
      time: new Date().toISOString(), traceparent: `00-${crypto.randomUUID().replace(/-/g, "")}-0000000000000001-01`,
      correlationId: crypto.randomUUID(), causationId: null, data: body } as Envelope<unknown>;
    await traza(raiz);
    try {
      const out = await ruta(body, raiz);
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(out));
    } catch (err) {
      res.writeHead(500, { "content-type": "application/problem+json" });
      res.end(JSON.stringify({ type: "about:blank", title: String(err), status: 500,
        traceId: raiz.traceparent.split("-")[1] }));
    }
  }).listen(port, () => console.log(`[${process.env.AXON_SERVICE}] escuchando en :${port}`));
}
