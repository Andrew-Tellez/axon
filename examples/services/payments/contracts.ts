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

/** Rutas HTTP que declara el manifiesto. El arranque debe fallar si
 *  alguna no tiene handler: un 404 en produccion no avisa a nadie. */
export const rutasHttp = ["POST /v1/payments", "POST /v1/payments/{paymentId}/refunds", "POST /v1/payouts"] as const;


/** Lado del teorema CAP declarado en el manifiesto: strong/reject.
 *  De ahi sale el nivel de aislamiento: pagar dos veces sale mas caro
 *  que reintentar, y servir un dato viejo cuesta menos que no servir. */
export const nivelAislamiento = "SERIALIZABLE" as const;


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


/** Todo lo que hace falta para alcanzar a otro servicio. Lo implementa quien
 *  despliega: HTTP, gRPC, un SDK. El framework no elige transporte. */
export interface Transporte {
  invocar(destino: string, metodo: string, cuerpo: unknown, cabeceras: Record<string, string>): Promise<unknown>;
}

export class ErrorAgotado extends Error {}
export class ErrorCircuitoAbierto extends Error {}

/** Politica declarada en el manifiesto. El generador la emite; nadie la teclea. */
export interface Politica {
  timeoutMs: number;
  reintentos: number;
  breaker: boolean;
}

/** Circuito por destino: cuando el otro lado se cae, deja de golpearlo.
 *  Tras el enfriamiento pasa a medio abierto y prueba una sola vez. */
class Circuito {
  #fallos = 0;
  #abiertoHasta = 0;
  // campos explicitos: las parameter properties son TS puro y no sobreviven
  // al type-stripping de Node
  readonly umbral: number;
  readonly enfriamientoMs: number;
  constructor(umbral = 5, enfriamientoMs = 10_000) {
    this.umbral = umbral;
    this.enfriamientoMs = enfriamientoMs;
  }

  permite(ahora: number) {
    return this.#abiertoHasta === 0 || ahora >= this.#abiertoHasta;
  }
  exito() {
    this.#fallos = 0;
    this.#abiertoHasta = 0;
  }
  fallo(ahora: number) {
    this.#fallos++;
    if (this.#fallos >= this.umbral) this.#abiertoHasta = ahora + this.enfriamientoMs;
  }
}

const circuitos = new Map<string, Circuito>();

const dormir = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function conTiempo<T>(p: Promise<T>, ms: number, quien: string): Promise<T> {
  let t: ReturnType<typeof setTimeout>;
  const limite = new Promise<never>((_, rechaza) => {
    t = setTimeout(() => rechaza(new ErrorAgotado(`${quien}: agotado tras ${ms}ms`)), ms);
  });
  try {
    return await Promise.race([p, limite]);
  } finally {
    clearTimeout(t!);
  }
}

/** Aplica la politica declarada. Los reintentos solo los emite el generador
 *  para metodos idempotentes: `axon verify` bloquea el resto. */
export async function conPolitica<T>(quien: string, pol: Politica, hacer: () => Promise<T>): Promise<T> {
  const circuito = pol.breaker
    ? (circuitos.get(quien) ?? circuitos.set(quien, new Circuito()).get(quien)!)
    : null;
  if (circuito && !circuito.permite(Date.now())) {
    throw new ErrorCircuitoAbierto(`${quien}: circuito abierto`);
  }
  let ultimo: unknown;
  for (let intento = 0; intento <= pol.reintentos; intento++) {
    try {
      const r = await conTiempo(hacer(), pol.timeoutMs, quien);
      circuito?.exito();
      return r;
    } catch (err) {
      ultimo = err;
      circuito?.fallo(Date.now());
      if (intento === pol.reintentos) break;
      // exponencial con jitter completo: sin jitter, todos los clientes
      // reintentan a la vez y el otro lado nunca se levanta
      const techo = Math.min(1000 * 2 ** intento, 10_000);
      await dormir(Math.random() * techo);
    }
  }
  throw ultimo;
}

/** Cabeceras de una llamada saliente: la traza sigue siendo la misma. */
export function cabeceras(e: Envelope<unknown>, idempotente: boolean): Record<string, string> {
  const h: Record<string, string> = {
    traceparent: e.traceparent,
    "x-correlation-id": e.correlationId,
    "x-causation-id": e.id,
  };
  // reintentar sin llave duplicaria el efecto en el otro lado
  if (idempotente) h["idempotency-key"] = e.id;
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


/** Clientes de las dependencias declaradas en el manifiesto. */
export class Clientes {
  protected readonly transporte: Transporte;
  constructor(transporte: Transporte) {
    this.transporte = transporte;
  }
  /** orders.getOrder · timeout 1000ms · 3 reintentos · breaker true */
  async ordersGetOrder(input: OrdersGetOrderIn, e: Envelope<unknown>): Promise<OrdersGetOrderOut> {
    const hacer = () => conPolitica("orders.getOrder", { timeoutMs: 1000, reintentos: 3, breaker: true }, async () =>
      (await this.transporte.invocar("orders", "getOrder", input, cabeceras(e, true))) as OrdersGetOrderOut);
    return hacer();
  }
  /** stripe.charges.create · timeout 8000ms · 0 reintentos · breaker true */
  async stripeChargesCreate(input: StripeChargesCreateIn, e: Envelope<unknown>): Promise<StripeChargesCreateOut> {
    const hacer = () => conPolitica("stripe.charges.create", { timeoutMs: 8000, reintentos: 0, breaker: true }, async () =>
      (await this.transporte.invocar("stripe", "charges.create", input, cabeceras(e, true))) as StripeChargesCreateOut);
    return hacer();
  }
}

