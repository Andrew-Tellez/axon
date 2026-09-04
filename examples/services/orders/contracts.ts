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
 *  cambio de estado, y un relay lo publica despues. Sin dual-write. */
export interface Outbox { stage(e: Envelope<unknown>): Promise<void>; }

/** Inbox / consumidor idempotente: el broker entrega al menos una vez, el
 *  efecto ocurre una sola. `once` no reejecuta un id ya visto. */
export interface Inbox { once(id: string, fn: () => Promise<void>): Promise<void>; }

export interface OrderPlacedV1 {
  orderId: string;
  customerId: string;
  total: { amount: number; currency: string };
}

export interface PlaceOrderIn {
  customerId: string;
  total: { amount: number; currency: string };
}

export interface PlaceOrderOut {
  orderId: string;
}

export interface GetOrderIn {
  orderId: string;
}

export interface GetOrderOut {
  orderId: string;
  status: string;
  total: { amount: number; currency: string };
}

export const manifest = {
  "service": "orders",
  "version": "1.0.0",
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
      "total": "money"
    }
  },
  "consumes": {},
  "methods": {
    "placeOrder": {
      "in": {
        "customerId": "uuid",
        "total": "money"
      },
      "out": {
        "orderId": "uuid"
      },
      "http": "POST /v1/orders",
      "idempotent": true,
      "auth": "public",
      "rate_limit": 60,
      "timeout_ms": 5000,
      "paginated": false
    },
    "getOrder": {
      "in": {
        "orderId": "uuid"
      },
      "out": {
        "orderId": "uuid",
        "status": "string",
        "total": "money"
      },
      "http": "GET /v1/orders/{orderId}",
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
  "machine": {},
  "infra": {
    "state": "postgres",
    "runtime": "container",
    "migrations": "sql/orders/",
    "secrets": [],
    "min_instances": null,
    "max_instances": null,
    "port": null,
    "buckets": {},
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
      "tenant_column": null,
      "tenant_exempt": []
    }
  }
} as const;

export abstract class OrdersService {
  protected readonly bus: Bus;
  protected readonly inbox: Inbox;
  constructor(bus: Bus, inbox: Inbox) {
    this.bus = bus;
    this.inbox = inbox;
  }
  static readonly wellKnown = "/.well-known/axon.json";
  protected emitOrderPlacedV1(data: OrderPlacedV1, cause?: Envelope<unknown>) {
    return this.bus.publish(newEnvelope("order.placed@v1", "orders", data, cause));
  }
  abstract placeOrder(input: PlaceOrderIn, e: Envelope<unknown>): Promise<PlaceOrderOut>;
  abstract getOrder(input: GetOrderIn, e: Envelope<unknown>): Promise<GetOrderOut>;
}


/** Lado del teorema CAP declarado en el manifiesto: eventual/degrade.
 *  De ahi sale el nivel de aislamiento: pagar dos veces sale mas caro
 *  que reintentar, y servir un dato viejo cuesta menos que no servir. */
export const nivelAislamiento = "READ COMMITTED" as const;
/** Presupuesto de obsolescencia: un dato mas viejo que esto no se sirve. */
export const obsolescenciaMaximaMs = 3000;


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

export interface PaymentsCapturePaymentIn {
  orderId: string;
  amount: { amount: number; currency: string };
}

export interface PaymentsCapturePaymentOut {
  paymentId: string;
}


/** Clientes de las dependencias declaradas en el manifiesto. */
export class Clientes {
  protected readonly transporte: Transporte;
  constructor(transporte: Transporte) {
    this.transporte = transporte;
  }
  /** payments.capturePayment · timeout 3000ms · 2 reintentos · breaker true */
  async paymentsCapturePayment(input: PaymentsCapturePaymentIn, e: Envelope<unknown>, respaldo: () => Promise<PaymentsCapturePaymentOut>): Promise<PaymentsCapturePaymentOut> {
    const hacer = () => conPolitica("payments.capturePayment", { timeoutMs: 3000, reintentos: 2, breaker: true }, async () =>
      (await this.transporte.invocar("payments", "capturePayment", input, cabeceras(e, true))) as PaymentsCapturePaymentOut);
    try {
      return await hacer();
    } catch {
      // declarado `degrade`: se sirve algo viejo antes que nada
      return respaldo();
    }
  }
}


/** Campos declarados PII en el manifiesto. */
export const camposPII = ["customer_email"] as const;

/** Reemplaza todo campo PII por "[redactado]", a cualquier profundidad.
 *  Pasa por aqui cualquier objeto antes de mandarlo a un log. */
export function redactar<T>(valor: T): T {
  if (Array.isArray(valor)) return valor.map(redactar) as T;
  if (valor === null || typeof valor !== "object") return valor;
  const salida: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(valor)) {
    salida[k] = (camposPII as readonly string[]).includes(k) ? "[redactado]" : redactar(v);
  }
  return salida as T;
}

