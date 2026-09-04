// generado por axon — no editar.
//
// Enchufalo desde tu propio archivo de pruebas:
//
//   import { pruebasDeContrato, pruebasDeMaquinas } from "./axon.testkit.ts";
//   import { Payments } from "./index.ts";
//   pruebasDeContrato((bus, inbox, outbox) => new Payments(bus, inbox, outbox));
//   pruebasDeMaquinas();
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  newEnvelope,
  type Envelope,
  type Bus,
  type Inbox,
  PaymentsService,
  type Outbox,
  type OrderPlacedV1,
  paymentTransitions,
  paymentNext,
  paymentCan,
  type PaymentState,
  type PaymentAction,
} from "./contracts.ts";

// Dobles en memoria. Deterministas y sin dependencias: las pruebas de
// contrato no necesitan infraestructura, las de integracion si.
export class BusFalso implements Bus {
  readonly publicados: Envelope<unknown>[] = [];
  async publish(e: Envelope<unknown>) {
    this.publicados.push(e);
  }
}

export class InboxEnMemoria implements Inbox {
  readonly vistos = new Set<string>();
  async once(id: string, fn: () => Promise<void>) {
    if (this.vistos.has(id)) return;
    this.vistos.add(id);
    await fn();
  }
}

export class OutboxFalso implements Outbox {
  readonly guardados: Envelope<unknown>[] = [];
  async stage(e: Envelope<unknown>) {
    this.guardados.push(e);
  }
}

// Fixtures derivadas del esquema que declara el DUENO de cada evento,
// no de lo que el consumidor cree recibir: ahi es donde aparece el drift.
export const fixtureOrderPlacedV1: OrderPlacedV1 = {
  orderId: "00000000-0000-4000-8000-000000000000",
  customerId: "00000000-0000-4000-8000-000000000000",
  total: { amount: 100, currency: "MXN" },
};

/** Pruebas de contrato. `crear` devuelve tu implementacion del servicio. */
export function pruebasDeContrato(crear: (bus: BusFalso, inbox: InboxEnMemoria, outbox: OutboxFalso) => PaymentsService) {
  const montar = () => {
    const bus = new BusFalso();
    const inbox = new InboxEnMemoria();
    const outbox = new OutboxFalso();
    const svc = crear(bus, inbox, outbox);
    return { svc, bus, inbox, outbox };
  };

  describe("payments · contrato", () => {
    it("acepta order.placed@v1 tal como lo emite su dueno", async () => {
      const { svc } = montar();
      await svc.dispatch(newEnvelope("order.placed@v1", "prueba", fixtureOrderPlacedV1));
    });

    it("la segunda entrega de order.placed@v1 no repite el efecto", async () => {
      const { svc, outbox } = montar();
      const e = newEnvelope("order.placed@v1", "prueba", fixtureOrderPlacedV1);
      await svc.dispatch(e);
      const despues = outbox.guardados.length;
      await svc.dispatch(e);
      assert.equal(outbox.guardados.length, despues, "el mismo envelope tuvo efecto dos veces");
    });
    it("propaga la cadena causal al reaccionar a order.placed@v1", async () => {
      const { svc, outbox } = montar();
      const causa = newEnvelope("order.placed@v1", "prueba", fixtureOrderPlacedV1);
      await svc.dispatch(causa);
      const salida = outbox.guardados;
      assert.ok(salida.length > 0, "no emitio nada");
      for (const e of salida) {
        assert.equal(e.causationId, causa.id, "causationId no apunta a la causa");
        assert.equal(e.correlationId, causa.correlationId, "se perdio el flujo");
        assert.equal(e.traceparent.split("-")[1], causa.traceparent.split("-")[1], "se perdio la traza");
      }
    });
    it("nada se publica fuera del outbox", async () => {
      const { bus } = montar();
      assert.equal(bus.publicados.length, 0, "dual-write: el handler toco el bus");
    });
  });
}

/** Pruebas de las maquinas de estado. No necesitan tu codigo. */
export function pruebasDeMaquinas() {
  describe("payments · maquina payment", () => {
    it("cada transicion declarada es legal desde sus estados de origen", () => {
      for (const [accion, t] of Object.entries(paymentTransitions)) {
        for (const desde of t.from) {
          assert.equal(paymentNext(desde, accion as PaymentAction), t.to);
          assert.ok(paymentCan(desde, accion as PaymentAction));
        }
      }
    });

    it("una transicion no declarada revienta", () => {
      const estados: PaymentState[] = ["pending", "captured", "failed", "refunded"];
      for (const [accion, t] of Object.entries(paymentTransitions)) {
        for (const e of estados.filter((s) => !t.from.includes(s))) {
          assert.throws(() => paymentNext(e, accion as PaymentAction));
          assert.equal(paymentCan(e, accion as PaymentAction), false);
        }
      }
    });
  });
}

