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
  const trace = cause ? cause.traceparent.split("-")[1] : hex(16);
  return {
    id: crypto.randomUUID(),
    type, source, data,
    time: new Date().toISOString(),
    traceparent: `00-${trace}-${hex(8)}-01`,
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
  "machine": {},
  "infra": {
    "state": "postgres",
    "runtime": "container",
    "migrations": "sql/orders/",
    "secrets": [],
    "min_instances": null,
    "max_instances": null,
    "port": null
  },
  "env": {
    "staging": {
      "state": null,
      "runtime": null,
      "migrations": null,
      "secrets": [],
      "min_instances": 0,
      "max_instances": null,
      "port": null
    },
    "prod": {
      "state": null,
      "runtime": null,
      "migrations": null,
      "secrets": [],
      "min_instances": 2,
      "max_instances": null,
      "port": null
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

