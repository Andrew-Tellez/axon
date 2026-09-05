// El coordinador de la saga. Lo unico escrito a mano son las ENTRADAS de cada
// paso —datos de negocio— y el diario. El orden, la compensacion en orden
// inverso y el barrido los genero axon.
import { CheckoutService, Clientes, rutasHttp, rutaBarridoCompra, correrCompra,
         barrerCompra, arrancarBarridoCompra,
         type CheckoutIn, type CheckoutOut, type CompraAcciones, type CompraSalidas,
         type Envelope, type SagaDiario, type SagaEstado, type Transporte } from "./contracts.ts";
import { arrancarTelemetria } from "../telemetria.ts";
import { esperarDb, servir } from "../runtime.ts";
import type pg from "pg";

/** HTTP contra los servicios declarados. El framework no elige transporte: el
 *  cliente generado pone el timeout, los reintentos y el circuito encima. */
function transporte(): Transporte {
  return {
    async invocar(destino, metodo, cuerpo, cabeceras) {
      const rutas: Record<string, string> = {
        capturePayment: "/v1/payments",
        refundPayment: `/v1/payments/${(cuerpo as any).paymentId}/refunds`,
        payoutMerchant: "/v1/payouts",
      };
      const r = await fetch(`http://${destino}:8080${rutas[metodo]}`, {
        method: "POST",
        headers: { "content-type": "application/json", ...cabeceras },
        body: JSON.stringify(cuerpo),
      });
      if (!r.ok) throw new Error(`${destino}.${metodo}: ${r.status} ${await r.text()}`);
      return r.json();
    },
  };
}

/** El diario sobre Postgres.
 *
 *  `reclamar` es la pieza que no se puede escribir ingenuamente: dos instancias
 *  barren a la vez, y si esto LISTARA las colgadas, las dos tomarian la misma y
 *  compensarian los mismos pasos. El reclamo y el filtro son la misma
 *  sentencia. */
class Diario implements SagaDiario {
  #db: pg.Pool;
  constructor(db: pg.Pool) {
    this.#db = db;
  }

  async abrir(id: string, _saga: string, e: Envelope<unknown>) {
    await this.#db.query(
      `INSERT INTO saga_compra (id, paso, estado, datos) VALUES ($1, 0, 'abierta', $2)
       ON CONFLICT (id) DO NOTHING`,
      [id, JSON.stringify(e)],
    );
  }

  async marcar(id: string, paso: number, estado: string, salida?: unknown) {
    // la salida se guarda en la MISMA sentencia que el estado: en dos, un corte
    // entre ellas deja el paso hecho y su resultado perdido
    await this.#db.query(
      // La clave del paso va como parametro APARTE del numero: usar el mismo
      // `$2` como int y como text deja el tipo ambiguo y Postgres lo rechaza.
      `UPDATE saga_compra
          SET paso = $2::int, estado = $3, actualizado = now(),
              salidas = CASE WHEN $5::jsonb IS NULL THEN salidas
                             ELSE coalesce(salidas, '{}'::jsonb) || jsonb_build_object($4::text, $5::jsonb) END
        WHERE id = $1`,
      [id, paso, estado, String(paso), salida === undefined ? null : JSON.stringify(salida)],
    );
  }

  async cerrar(id: string, estado: SagaEstado) {
    await this.#db.query(
      `UPDATE saga_compra SET estado = $2, actualizado = now() WHERE id = $1`,
      [id, estado],
    );
  }

  async leer(id: string) {
    const { rows } = await this.#db.query(
      `SELECT paso, estado, coalesce(salidas, '{}'::jsonb) AS salidas
         FROM saga_compra WHERE id = $1 AND estado <> 'abierta'`,
      [id],
    );
    if (!rows[0]) return null;
    return {
      paso: Number(rows[0].paso),
      estado: rows[0].estado as string,
      salidas: rows[0].salidas as Record<number, unknown>,
    };
  }

  async reclamar(_saga: string, antesDe: Date, limite: number) {
    const { rows } = await this.#db.query(
      `UPDATE saga_compra SET actualizado = now()
        WHERE id IN (
          SELECT id FROM saga_compra
           WHERE estado IN ('abierta','intentando','hecho') AND actualizado < $1
           ORDER BY actualizado
           LIMIT $2
           FOR UPDATE SKIP LOCKED
        )
        RETURNING id, datos`,
      [antesDe, limite],
    );
    return rows.map((r: any) => ({ id: r.id as string, datos: r.datos as Envelope<unknown> }));
  }
}

/** Las entradas de cada paso: lo unico que el generador no puede saber. El
 *  orden lo sabe el coordinador; el contenido, no.
 *
 *  Todo sale del envelope y de `previas`, nunca de una variable de este
 *  proceso: el barrido corre estas mismas acciones sobre una saga que arranco
 *  en OTRO proceso, y ahi una closure no existe. */
function acciones(clientes: Clientes): CompraAcciones {
  const entrada = (e: Envelope<unknown>) => e.data as CheckoutIn;
  return {
    async paso1CapturePayment(e) {
      const { orderId, amount } = entrada(e);
      return clientes.paymentsCapturePayment({ orderId, amount }, e);
    },
    async deshacer1RefundPayment(e, previas) {
      // sin cobro no hay nada que deshacer, y eso no es un error: el
      // coordinador compensa tambien el paso que quedo en duda
      if (!previas.paso1) return;
      await clientes.paymentsRefundPayment({ paymentId: previas.paso1.paymentId }, e);
    },
    async paso2PayoutMerchant(e, previas) {
      if (!previas.paso1) throw new Error("no hay cobro que pagar");
      return clientes.paymentsPayoutMerchant(
        { paymentId: previas.paso1.paymentId, amount: entrada(e).amount },
        e,
      );
    },
  };
}

class Checkout extends CheckoutService {
  #diario: SagaDiario;
  #acciones: CompraAcciones;
  constructor(db: pg.Pool) {
    // Ni bus ni inbox: este servicio no emite ni consume. El generador solo
    // pide los colaboradores que el manifiesto declara usar.
    super();
    this.#diario = new Diario(db);
    this.#acciones = acciones(new Clientes(transporte()));
  }

  get diario() {
    return this.#diario;
  }
  get pasos() {
    return this.#acciones;
  }

  async checkout(_input: CheckoutIn, e: Envelope<unknown>): Promise<CheckoutOut> {
    // el id de la saga es el del flujo: el diario y la traza hablan del mismo
    // hecho, y `axon trace` los cruza sin adivinar
    const r = await correrCompra(e.correlationId, this.#acciones, this.#diario, e);
    return { estado: r.estado };
  }
}

if (process.env.NODE_TEST_CONTEXT === undefined) await main();

async function main() {
  arrancarTelemetria();
  const db = await esperarDb();
  const svc = new Checkout(db);

  // Dentro del proceso Y por la ruta que golpea el programador que despliega
  // `axon infra`: un servicio escalado a cero no barre nada por su cuenta.
  arrancarBarridoCompra(svc.pasos, svc.diario, (r) => {
    if (r.reclamadas) console.log(`[checkout] barrido ${JSON.stringify(r)}`);
    if (r.atascadas) console.error(`[checkout] ${r.atascadas} sagas ATASCADAS`);
  });

  servir(
    Number(process.env.PORT ?? 8080),
    {
      "POST /v1/checkouts": (body, e) => svc.checkout(body, e),
      [rutaBarridoCompra]: () => barrerCompra(svc.pasos, svc.diario),
    },
    rutasHttp,
  );
}
