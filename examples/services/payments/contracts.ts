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

export interface PaymentCapturedV1 {
  paymentId: string;
  orderId: string;
  amount: { amount: number; currency: string };
}

// order.placed@v1: esquema declarado por orders, su dueno
export interface OrderPlacedV1 {
  orderId: string;
  customerId: string;
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

export const manifest = {
  "service": "payments",
  "version": "1.2.0",
  "owner": "equipo-pagos",
  "tier": "0",
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
  "infra": {
    "state": "postgres",
    "runtime": "container",
    "migrations": "sql/payments/",
    "secrets": [
      "STRIPE_API_KEY"
    ],
    "min_instances": 1,
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
      "min_instances": 3,
      "max_instances": null,
      "port": null
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
  protected emitPaymentCapturedV1(data: PaymentCapturedV1, cause?: Envelope<unknown>) {
    return this.outbox.stage(newEnvelope("payment.captured@v1", "payments", data, cause));
  }
  /** consume order.placed@v1 */
  abstract onOrderPlaced(e: Envelope<OrderPlacedV1>): Promise<void>;
  abstract capturePayment(input: CapturePaymentIn, e: Envelope<unknown>): Promise<CapturePaymentOut>;
  abstract refundPayment(input: RefundPaymentIn, e: Envelope<unknown>): Promise<RefundPaymentOut>;
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
