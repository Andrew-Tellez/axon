// generado por axon desde orders.toml — no editar


/** Envelope CloudEvents + cadena causal. La trazabilidad no es opcional:
 *  ningun mensaje sale del proceso sin traceparent, correlationId y causationId. */
export interface Envelope<T> {
  id: string;
  type: string;
  source: string;
  time: string;
  traceparent: string;        // W3C: 00-<trace>-<span>-<flags>
  correlationId: string;      // estable en todo el flujo de negocio
  causationId: string | null; // id del mensaje que provoco este
  data: T;
}

const hex = (n: number) =>
  Array.from(crypto.getRandomValues(new Uint8Array(n)), b => b.toString(16).padStart(2, "0")).join("");

export function newEnvelope<T>(type: string, source: string, data: T, cause?: Envelope<unknown>): Envelope<T> {
  const partes = cause?.traceparent.split("-");
  const trace = partes?.[1] ?? hex(16);
  // Los flags se heredan, no se inventan: declarar "muestreado" sobre una traza
  // que no lo esta deja fragmentos colgando de un padre que nunca se exporto.
  const flags = partes?.[3] ?? "01";
  return {
    id: crypto.randomUUID(),
    type, source, data,
    time: new Date().toISOString(),
    traceparent: `00-${trace}-${hex(8)}-${flags}`,
    correlationId: cause ? cause.correlationId : crypto.randomUUID(),
    causationId: cause ? cause.id : null,
  };
}

export interface Bus { publish(e: Envelope<unknown>): Promise<void>; }

/** Transactional outbox: el evento se guarda en la misma transaccion que el
 *  cambio de estado, y un relay lo publica despues.
 *
 *  `tx` es la transaccion de QUIEN LLAMA, y es obligatoria. Con una conexion
 *  propia, un `stage` se confirma solo: si la transaccion de quien llama se
 *  revierte, el cambio de estado no ocurre y el evento SI, y el relay publica
 *  algo que nunca paso. Eso es exactamente el dual-write que el outbox
 *  existe para evitar, y no se ve en ninguna parte hasta que alguien pregunta
 *  por un evento sin su fila.
 *
 *  El tipo queda abierto porque el framework no elige cliente de base. */
export interface Outbox<Tx = unknown> { stage(e: Envelope<unknown>, tx: Tx): Promise<void>; }

/** Inbox / consumidor idempotente: el broker entrega al menos una vez, el
 *  efecto ocurre una sola. `once` no reejecuta un id ya visto. */
export interface Inbox { once(id: string, fn: () => Promise<void>): Promise<void>; }

export interface OrderPlacedV1 {
  orderId: string;
  customerId: string;
  customerEmail: string;
  total: { amount: number; currency: string };
}

export interface PlaceOrderIn {
  tenantId: string;
  customerId: string;
  customerEmail: string;
  total: { amount: number; currency: string };
}

export interface PlaceOrderOut {
  orderId: string;
}

export interface GetOrderIn {
  tenantId: string;
  orderId: string;
}

export interface GetOrderOut {
  orderId: string;
  status: string;
  total: { amount: number; currency: string };
}

export const manifest = {
  "service": "orders",
  "version": "2.0.0",
  "owner": "equipo-comercio",
  "tier": "1",
  "pii": [
    "customer_email"
  ],
  "external": false,
  "emits": {
    "order.placed@v1": {
      "orderId": "uuid",
      "customerId": "uuid",
      "customerEmail": "string",
      "total": "money"
    }
  },
  "consumes": {},
  "methods": {
    "placeOrder": {
      "in": {
        "tenantId": "uuid",
        "customerId": "uuid",
        "customerEmail": "string",
        "total": "money"
      },
      "out": {
        "orderId": "uuid"
      },
      "http": "POST /v1/tenants/{tenantId}/orders",
      "idempotent": true,
      "auth": "public",
      "rate_limit": 60,
      "timeout_ms": 5000,
      "paginated": false
    },
    "getOrder": {
      "in": {
        "tenantId": "uuid",
        "orderId": "uuid"
      },
      "out": {
        "orderId": "uuid",
        "status": "string",
        "total": "money"
      },
      "http": "GET /v1/tenants/{tenantId}/orders/{orderId}",
      "idempotent": false,
      "auth": "required",
      "rate_limit": null,
      "timeout_ms": 2000,
      "paginated": false
    }
  },
  "depends": [
    {
      "service": "payments",
      "external": null,
      "method": "capturePayment",
      "via": null,
      "timeout_ms": 3000,
      "retries": 2,
      "breaker": true
    }
  ],
  "patterns": {
    "outbox": false
  },
  "cap": {
    "consistency": "eventual",
    "on_partition": "degrade",
    "max_staleness_ms": 3000
  },
  "flags": {},
  "analytics": {
    "export": true,
    "pii": "hash",
    "warehouse": "clickhouse"
  },
  "pooler": {
    "engine": "pgdog",
    "mode": "transaction",
    "shards": 4,
    "max_client_conn": 500,
    "pool_size": 40,
    "cross_shard_disabled": true,
    "tenant_binding": "set_local"
  },
  "machine": {},
  "saga": {},
  "aggregate": {},
  "view": {},
  "infra": {
    "state": "postgres",
    "runtime": "container",
    "migrations": "sql/orders/",
    "secrets": [],
    "min_instances": null,
    "max_instances": null,
    "port": null,
    "buckets": {},
    "pool_size": 4,
    "max_connections": 100,
    "ha": null,
    "backup_retention_days": 14,
    "pitr": null,
    "read_replicas": 2,
    "shard_key": "tenant_id",
    "tenant_column": "tenant_id",
    "tenant_exempt": []
  },
  "env": {
    "staging": {
      "state": null,
      "runtime": null,
      "migrations": null,
      "secrets": [],
      "min_instances": 0,
      "max_instances": null,
      "port": null,
      "buckets": {},
      "pool_size": null,
      "max_connections": null,
      "ha": null,
      "backup_retention_days": null,
      "pitr": null,
      "read_replicas": null,
      "shard_key": null,
      "tenant_column": null,
      "tenant_exempt": []
    },
    "prod": {
      "state": null,
      "runtime": null,
      "migrations": null,
      "secrets": [],
      "min_instances": 2,
      "max_instances": null,
      "port": null,
      "buckets": {},
      "pool_size": null,
      "max_connections": null,
      "ha": null,
      "backup_retention_days": null,
      "pitr": null,
      "read_replicas": null,
      "shard_key": null,
      "tenant_column": null,
      "tenant_exempt": []
    }
  }
} as const;

export abstract class OrdersService {
  protected readonly bus: Bus;
  constructor(bus: Bus) {
    this.bus = bus;
  }
  static readonly wellKnown = "/.well-known/axon.json";
  protected emitOrderPlacedV1(data: OrderPlacedV1, cause?: Envelope<unknown>) {
    return this.bus.publish(newEnvelope("order.placed@v1", "orders", data, cause));
  }
  abstract placeOrder(input: PlaceOrderIn, e: Envelope<unknown>): Promise<PlaceOrderOut>;
  abstract getOrder(input: GetOrderIn, e: Envelope<unknown>): Promise<GetOrderOut>;
}


/** HTTP routes the manifest declares. Startup must fail if any of them
 *  has no handler: a 404 in production tells nobody. */
export const httpRoutes = ["POST /v1/tenants/{tenantId}/orders", "GET /v1/tenants/{tenantId}/orders/{orderId}"] as const;


/** The CAP side declared in the manifest: eventual/degrade.
 *  The isolation level follows from it: paying twice costs more than
 *  retrying, and serving stale data costs less than serving nothing. */
export const isolationLevel = "READ COMMITTED" as const;
/** Staleness budget: data older than this does not get served. */
export const maxStalenessMs = 3000;



/** Everything needed to reach another service. Implemented by whoever
 *  deploys: HTTP, gRPC, an SDK. The framework does not pick a transport. */
export interface Transport {
  call(target: string, method: string, body: unknown, headers: Record<string, string>): Promise<unknown>;
}

export class TimedOut extends Error {}
export class CircuitOpen extends Error {}

/** The policy declared in the manifest. The generator emits it; nobody types it. */
export interface Policy {
  timeoutMs: number;
  retries: number;
  breaker: boolean;
}

/** One breaker per target: when the other side goes down, stop hitting it.
 *  After the cooldown it goes half-open and tries exactly once. */
class Breaker {
  #failures = 0;
  #openUntil = 0;
  // explicit fields: parameter properties are TypeScript-only and do not
  // survive Node's type stripping
  readonly threshold: number;
  readonly cooldownMs: number;
  constructor(threshold = 5, cooldownMs = 10_000) {
    this.threshold = threshold;
    this.cooldownMs = cooldownMs;
  }

  allows(now: number) {
    return this.#openUntil === 0 || now >= this.#openUntil;
  }
  succeeded() {
    this.#failures = 0;
    this.#openUntil = 0;
  }
  failed(now: number) {
    this.#failures++;
    if (this.#failures >= this.threshold) this.#openUntil = now + this.cooldownMs;
  }
}

const breakers = new Map<string, Breaker>();

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function withTimeout<T>(p: Promise<T>, ms: number, who: string): Promise<T> {
  let t: ReturnType<typeof setTimeout>;
  const limit = new Promise<never>((_, reject) => {
    t = setTimeout(() => reject(new TimedOut(`${who}: timed out after ${ms}ms`)), ms);
  });
  try {
    return await Promise.race([p, limit]);
  } finally {
    clearTimeout(t!);
  }
}

/** Applies the declared policy. Retries are only emitted for idempotent
 *  methods: `axon verify` blocks the rest. */
export async function withPolicy<T>(who: string, pol: Policy, attempt: () => Promise<T>): Promise<T> {
  const breaker = pol.breaker
    ? (breakers.get(who) ?? breakers.set(who, new Breaker()).get(who)!)
    : null;
  if (breaker && !breaker.allows(Date.now())) {
    throw new CircuitOpen(`${who}: circuit open`);
  }
  let last: unknown;
  for (let n = 0; n <= pol.retries; n++) {
    try {
      const r = await withTimeout(attempt(), pol.timeoutMs, who);
      breaker?.succeeded();
      return r;
    } catch (err) {
      last = err;
      breaker?.failed(Date.now());
      if (n === pol.retries) break;
      // exponential with full jitter: without jitter every client retries at
      // the same instant and the other side never comes back up
      const ceiling = Math.min(1000 * 2 ** n, 10_000);
      await sleep(Math.random() * ceiling);
    }
  }
  throw last;
}

/** Headers of an outgoing call: the trace stays the same one. */
export function headers(e: Envelope<unknown>, idempotent: boolean): Record<string, string> {
  const h: Record<string, string> = {
    traceparent: e.traceparent,
    "x-correlation-id": e.correlationId,
    "x-causation-id": e.id,
  };
  // retrying without a key would duplicate the effect on the other side
  if (idempotent) h["idempotency-key"] = e.id;
  return h;
}

export interface PaymentsCapturePaymentIn {
  orderId: string;
  amount: { amount: number; currency: string };
}

export interface PaymentsCapturePaymentOut {
  paymentId: string;
}


/** Clients for the dependencies declared in the manifest. */
export class Clients {
  protected readonly transport: Transport;
  constructor(transport: Transport) {
    this.transport = transport;
  }
  /** payments.capturePayment · timeout 3000ms · 2 retries · breaker true */
  async paymentsCapturePayment(input: PaymentsCapturePaymentIn, e: Envelope<unknown>, fallback: () => Promise<PaymentsCapturePaymentOut>): Promise<PaymentsCapturePaymentOut> {
    const attempt = () => withPolicy("payments.capturePayment", { timeoutMs: 3000, retries: 2, breaker: true }, async () =>
      (await this.transport.call("payments", "capturePayment", input, headers(e, true))) as PaymentsCapturePaymentOut);
    try {
      return await attempt();
    } catch {
      // declared `degrade`: serving something stale beats serving nothing
      return fallback();
    }
  }
}


/** Fields declared as PII in the manifest. */
export const piiFields = ["customer_email"] as const;

/** Replaces every PII field with "[redacted]", at any depth.
 *  Run anything through here before it reaches a log.
 *
 *  The comparison normalizes: `customer_email` declared in the manifest
 *  covers `customerEmail` in a contract and `customer-email` in a header.
 *  The same concept gets declared once. */
const normalizePii = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, "");
const pii = new Set((piiFields as readonly string[]).map(normalizePii));

export function redact<T>(value: T): T {
  if (Array.isArray(value)) return value.map(redact) as T;
  if (value === null || typeof value !== "object") return value;
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(value)) {
    out[k] = pii.has(normalizePii(k)) ? "[redacted]" : redact(v);
  }
  return out as T;
}

