// generado por axon desde payments.toml — no editar


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

export interface PaymentCapturedV1 {
  paymentId: string;
  orderId: string;
  amount: { amount: number; currency: string };
}

// order.placed@v1: esquema declarado por orders, su dueno
export interface OrderPlacedV1 {
  orderId: string;
  customerId: string;
  customerEmail: string;
  total: { amount: number; currency: string };
}

export interface CapturePaymentIn {
  orderId: string;
  amount: { amount: number; currency: string };
}

export interface CapturePaymentOut {
  paymentId: string;
}

export interface RefundPaymentIn {
  paymentId: string;
}

export interface RefundPaymentOut {
  paymentId: string;
  status: string;
}

export interface PayoutMerchantIn {
  paymentId: string;
  amount: { amount: number; currency: string };
}

export interface PayoutMerchantOut {
  payoutId: string;
}

export const manifest = {
  "service": "payments",
  "version": "1.2.0",
  "owner": "equipo-pagos",
  "tier": "0",
  "pii": [],
  "external": false,
  "emits": {
    "payment.captured@v1": {
      "paymentId": "uuid",
      "orderId": "uuid",
      "amount": "money"
    }
  },
  "consumes": {
    "order.placed@v1": {
      "handler": "onOrderPlaced"
    }
  },
  "methods": {
    "capturePayment": {
      "in": {
        "orderId": "uuid",
        "amount": "money"
      },
      "out": {
        "paymentId": "uuid"
      },
      "http": "POST /v1/payments",
      "idempotent": true,
      "auth": "required",
      "rate_limit": null,
      "timeout_ms": 8000,
      "paginated": false
    },
    "refundPayment": {
      "in": {
        "paymentId": "uuid"
      },
      "out": {
        "paymentId": "uuid",
        "status": "string"
      },
      "http": "POST /v1/payments/{paymentId}/refunds",
      "idempotent": true,
      "auth": "required",
      "rate_limit": null,
      "timeout_ms": 8000,
      "paginated": false
    },
    "payoutMerchant": {
      "in": {
        "paymentId": "uuid",
        "amount": "money"
      },
      "out": {
        "payoutId": "uuid"
      },
      "http": "POST /v1/payouts",
      "idempotent": true,
      "auth": "required",
      "rate_limit": null,
      "timeout_ms": 4000,
      "paginated": false
    }
  },
  "depends": [
    {
      "service": "orders",
      "external": null,
      "method": "getOrder",
      "via": "onOrderPlaced",
      "timeout_ms": 1000,
      "retries": 3,
      "breaker": true
    },
    {
      "service": null,
      "external": "stripe",
      "method": "charges.create",
      "via": null,
      "timeout_ms": 8000,
      "retries": 0,
      "breaker": true
    }
  ],
  "patterns": {
    "outbox": true
  },
  "cap": {
    "consistency": "strong",
    "on_partition": "reject",
    "max_staleness_ms": null
  },
  "flags": {
    "cobro_v2": {
      "owner": "equipo-pagos",
      "variants": {},
      "default_variant": null,
      "expires": "2026-12-31",
      "default": false,
      "rollout": 10,
      "sticky_by": "tenant_id",
      "kill_switch": false
    },
    "cortar_stripe": {
      "owner": "equipo-pagos",
      "variants": {},
      "default_variant": null,
      "expires": null,
      "default": false,
      "rollout": null,
      "sticky_by": null,
      "kill_switch": true
    },
    "proveedor_de_cobro": {
      "owner": "equipo-pagos",
      "variants": {
        "stripe": "stripe",
        "adyen": "adyen"
      },
      "default_variant": "stripe",
      "expires": "2027-06-30",
      "default": false,
      "rollout": 20,
      "sticky_by": "tenant_id",
      "kill_switch": false
    },
    "limite_de_reintentos": {
      "owner": "equipo-pagos",
      "variants": {
        "normal": 3,
        "degradado": 0
      },
      "default_variant": "normal",
      "expires": null,
      "default": false,
      "rollout": null,
      "sticky_by": null,
      "kill_switch": true
    }
  },
  "analytics": {
    "export": true,
    "pii": "exclude",
    "warehouse": "clickhouse"
  },
  "pooler": {
    "engine": "none",
    "mode": "session",
    "shards": 1,
    "max_client_conn": null,
    "pool_size": null,
    "cross_shard_disabled": true,
    "tenant_binding": null
  },
  "machine": {
    "payment": {
      "initial": "pending",
      "final": [
        "refunded",
        "failed"
      ],
      "transitions": {
        "capture": {
          "from": [
            "pending"
          ],
          "to": "captured",
          "on": "capturePayment",
          "emits": "payment.captured@v1",
          "compensates": null
        },
        "fail": {
          "from": [
            "pending"
          ],
          "to": "failed",
          "on": "capturePayment",
          "emits": null,
          "compensates": null
        },
        "refund": {
          "from": [
            "captured"
          ],
          "to": "refunded",
          "on": "refundPayment",
          "emits": null,
          "compensates": "capture"
        }
      }
    }
  },
  "saga": {},
  "aggregate": {},
  "view": {},
  "infra": {
    "state": "postgres",
    "runtime": "container",
    "migrations": "sql/payments/",
    "secrets": [
      "STRIPE_API_KEY"
    ],
    "min_instances": 1,
    "max_instances": null,
    "port": null,
    "buckets": {
      "recibos": {
        "public": false,
        "retention_days": 2555,
        "cache_ttl": null
      },
      "assets": {
        "public": true,
        "retention_days": 365,
        "cache_ttl": 86400
      }
    },
    "pool_size": 8,
    "max_connections": 200,
    "ha": true,
    "backup_retention_days": 30,
    "pitr": true,
    "read_replicas": null,
    "shard_key": null,
    "tenant_column": "tenant_id",
    "tenant_exempt": [
      "intento"
    ]
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
      "min_instances": 3,
      "max_instances": 40,
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

export abstract class PaymentsService {
  protected readonly bus: Bus;
  protected readonly inbox: Inbox;
  protected readonly outbox: Outbox;
  constructor(bus: Bus, inbox: Inbox, outbox: Outbox) {
    this.bus = bus;
    this.inbox = inbox;
    this.outbox = outbox;
  }
  static readonly wellKnown = "/.well-known/axon.json";
  protected emitPaymentCapturedV1(data: PaymentCapturedV1, tx: unknown, cause?: Envelope<unknown>) {
    return this.outbox.stage(newEnvelope("payment.captured@v1", "payments", data, cause), tx);
  }
  /** consume order.placed@v1 */
  abstract onOrderPlaced(e: Envelope<OrderPlacedV1>): Promise<void>;
  abstract capturePayment(input: CapturePaymentIn, e: Envelope<unknown>): Promise<CapturePaymentOut>;
  abstract refundPayment(input: RefundPaymentIn, e: Envelope<unknown>): Promise<RefundPaymentOut>;
  abstract payoutMerchant(input: PayoutMerchantIn, e: Envelope<unknown>): Promise<PayoutMerchantOut>;
  /** Punto de entrada unico: rutea por tipo y deduplica por id de envelope. */
  dispatch(e: Envelope<unknown>): Promise<void> {
    return this.inbox.once(e.id, async () => {
      switch (e.type) {
        case "order.placed@v1": return this.onOrderPlaced(e as Envelope<OrderPlacedV1>);
        default: throw new Error(`payments: tipo no declarado en el manifiesto: ${e.type}`);
      }
    });
  }
}

export type PaymentState = "pending" | "captured" | "failed" | "refunded";
export type PaymentAction = "capture" | "fail" | "refund";
export const paymentFinal: readonly PaymentState[] = ["refunded", "failed"];
/** Transiciones declaradas en el manifiesto. Generado: no editar. */
export const paymentTransitions: Record<PaymentAction, { from: readonly PaymentState[]; to: PaymentState; on: string }> = {
  capture: { from: ["pending"], to: "captured", on: "capturePayment" },
  fail: { from: ["pending"], to: "failed", on: "capturePayment" },
  refund: { from: ["captured"], to: "refunded", on: "refundPayment" },
};
export function paymentNext(state: PaymentState, action: PaymentAction): PaymentState {
  const t = paymentTransitions[action];
  if (!t.from.includes(state)) throw new Error(`payment: ${action} no es legal desde ${state}`);
  return t.to;
}
export const paymentCan = (state: PaymentState, action: PaymentAction) => paymentTransitions[action].from.includes(state);

/** HTTP routes the manifest declares. Startup must fail if any of them
 *  has no handler: a 404 in production tells nobody. */
export const httpRoutes = ["POST /v1/payments", "POST /v1/payments/{paymentId}/refunds", "POST /v1/payouts"] as const;


/** The CAP side declared in the manifest: strong/reject.
 *  The isolation level follows from it: paying twice costs more than
 *  retrying, and serving stale data costs less than serving nothing. */
export const isolationLevel = "SERIALIZABLE" as const;


/** Proveedor de flags, con la forma de OpenFeature: `evaluar` recibe el
 *  nombre, el valor por defecto y el contexto por el que se fija. Los cuatro
 *  tipos del estandar, para que el SDK real encaje sin traduccion. */
export interface Flags {
  evaluar<T extends boolean | string | number | object>(
    nombre: string,
    porDefecto: T,
    contexto: Record<string, string>,
  ): Promise<T>;
}

/** `cobro_v2`: boolean de OpenFeature.
 *  Se fija por `tenant_id`: la misma entidad toma siempre el mismo camino.
 */
export const flagCobroV2 = (flags: Flags, tenant_id: string): Promise<boolean> =>
  flags.evaluar("cobro_v2", false, { targetingKey: tenant_id, tenant_id });

/** `cortar_stripe`: boolean de OpenFeature.
 */
export const flagCortarStripe = (flags: Flags): Promise<boolean> =>
  flags.evaluar("cortar_stripe", false, {});

/** `proveedor_de_cobro`: string de OpenFeature.
 *  Variantes: `stripe` = "stripe", `adyen` = "adyen".
 *  Se fija por `tenant_id`: la misma entidad toma siempre el mismo camino.
 */
export const flagProveedorDeCobro = (flags: Flags, tenant_id: string): Promise<string> =>
  flags.evaluar("proveedor_de_cobro", "stripe", { targetingKey: tenant_id, tenant_id });

/** `limite_de_reintentos`: number de OpenFeature.
 *  Variantes: `normal` = 3, `degradado` = 0.
 */
export const flagLimiteDeReintentos = (flags: Flags): Promise<number> =>
  flags.evaluar("limite_de_reintentos", 3, {});

/** Los flags que declara el manifiesto. Un flag que no esta aca no existe. */
export const flagsDeclarados = ["cobro_v2", "cortar_stripe", "proveedor_de_cobro", "limite_de_reintentos"] as const;


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

export interface OrdersGetOrderIn {
  tenantId: string;
  orderId: string;
}

export interface OrdersGetOrderOut {
  orderId: string;
  status: string;
  total: { amount: number; currency: string };
}

export interface StripeChargesCreateIn {
  amount: number;
  currency: string;
  source: string;
}

export interface StripeChargesCreateOut {
  id: string;
  status: string;
}


/** Clients for the dependencies declared in the manifest. */
export class Clients {
  protected readonly transport: Transport;
  constructor(transport: Transport) {
    this.transport = transport;
  }
  /** orders.getOrder · timeout 1000ms · 3 retries · breaker true */
  async ordersGetOrder(input: OrdersGetOrderIn, e: Envelope<unknown>): Promise<OrdersGetOrderOut> {
    const attempt = () => withPolicy("orders.getOrder", { timeoutMs: 1000, retries: 3, breaker: true }, async () =>
      (await this.transport.call("orders", "getOrder", input, headers(e, true))) as OrdersGetOrderOut);
    return attempt();
  }
  /** stripe.charges.create · timeout 8000ms · 0 retries · breaker true */
  async stripeChargesCreate(input: StripeChargesCreateIn, e: Envelope<unknown>): Promise<StripeChargesCreateOut> {
    const attempt = () => withPolicy("stripe.charges.create", { timeoutMs: 8000, retries: 0, breaker: true }, async () =>
      (await this.transport.call("stripe", "charges.create", input, headers(e, true))) as StripeChargesCreateOut);
    return attempt();
  }
}

