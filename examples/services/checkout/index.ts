// El coordinador de la saga. Lo unico escrito a mano son las ENTRADAS de cada
// paso —datos de negocio— y el diario. El orden, la compensacion en orden
// inverso y el barrido los genero axon.
import { CheckoutService, Clientes, rutasHttp, rutaBarridoCompra, correrCompra,
         barrerCompra, arrancarBarridoCompra, compraFold, conversionAplicar,
         newEnvelope, VersionEnConflicto,
         type CheckoutIn, type CheckoutOut, type CompraAcciones, type CompraSalidas,
         type CompraReglas, type ConversionProyeccion, type Checkpoint,
         type Envelope, type FlujoEventos, type SagaDiario, type SagaEstado,
         type Transporte } from "./contracts.ts";
import { arrancarTelemetria } from "../telemetria.ts";
import { bus, conectar, esperarDb, servir } from "../runtime.ts";
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

/** El flujo de eventos sobre Postgres.
 *
 *  `append` recibe la version que el llamador creia vigente. El UNIQUE
 *  (stream_id, version) es lo que la hace valer: si otro escribio en medio, la
 *  insercion falla y quien la recibe vuelve a leer. Sin el UNIQUE las dos
 *  entrarian y el estado reconstruido dependeria del orden de lectura. */
class Flujo implements FlujoEventos {
  #db: pg.Pool;
  constructor(db: pg.Pool) {
    this.#db = db;
  }

  async leer(streamId: string, desde = 0) {
    const { rows } = await this.#db.query(
      `SELECT version, type, data FROM compra_event
        WHERE stream_id = $1 AND version > $2 ORDER BY version`,
      [streamId, desde],
    );
    return rows.map((r: any) => ({
      version: Number(r.version),
      type: r.type as string,
      data: r.data as unknown,
    }));
  }

  async append(streamId: string, esperada: number, e: Envelope<unknown>) {
    const version = esperada + 1;
    try {
      await this.#db.query(
        `INSERT INTO compra_event (id, stream_id, version, type, data)
         VALUES ($1,$2,$3,$4,$5)`,
        [e.id, streamId, version, e.type, JSON.stringify(e.data)],
      );
    } catch (err: any) {
      // 23505: unique_violation. Otro escribio esta version primero, y eso no
      // es un error de programa: es la condicion normal de dos usuarios sobre
      // el mismo agregado.
      if (err?.code === "23505") throw new VersionEnConflicto(streamId, esperada);
      throw err;
    }
    return version;
  }
}

/** Las reglas del dominio: como cada evento cambia el estado. Es lo unico que
 *  el generador no puede saber, y un evento declarado sin su caso no compila. */
interface Compra {
  estado: "nueva" | "iniciada" | "cobrada" | "compensada";
  centavos: number;
  paymentId: string | null;
}

const reglasCompra: CompraReglas<Compra> = {
  inicial: () => ({ estado: "nueva", centavos: 0, paymentId: null }),
  aplicarCompraIniciadaV1: (s, e) => ({ ...s, estado: "iniciada", centavos: e.amount.amount }),
  aplicarCompraCobradaV1: (s, e) => ({ ...s, estado: "cobrada", paymentId: e.paymentId }),
  aplicarCompraCompensadaV1: (s) => ({ ...s, estado: "compensada" }),
};

/** La proyeccion. Cada `aplicar*` guarda la posicion en la MISMA transaccion
 *  que su efecto: en dos, un corte entre ellas deja la vista adelantada o
 *  atrasada respecto de lo que dice haber aplicado, y ninguna da un error. */
class Conversion implements ConversionProyeccion, Checkpoint {
  #db: pg.Pool;
  constructor(db: pg.Pool) {
    this.#db = db;
  }

  async leer(vista: string) {
    const { rows } = await this.#db.query(
      `SELECT posicion FROM vista_conversion_checkpoint WHERE vista = $1`,
      [vista],
    );
    return rows[0] ? Number(rows[0].posicion) : 0;
  }

  async #enUna(posicion: number, sql: string, args: unknown[]) {
    const c = await this.#db.connect();
    try {
      await c.query("BEGIN");
      await c.query(sql, args);
      await c.query(
        `INSERT INTO vista_conversion_checkpoint (vista, posicion) VALUES ('conversion', $1)
         ON CONFLICT (vista) DO UPDATE SET posicion = GREATEST(vista_conversion_checkpoint.posicion, $1)`,
        [posicion],
      );
      await c.query("COMMIT");
    } catch (err) {
      await c.query("ROLLBACK").catch(() => {});
      throw err;
    } finally {
      c.release();
    }
  }

  async aplicarCompraIniciadaV1(e: Envelope<any>, posicion: number) {
    await this.#enUna(
      posicion,
      `INSERT INTO vista_conversion (stream_id, estado, centavos, evento_en)
       VALUES ($1,'iniciada',$2,$3)
       ON CONFLICT (stream_id) DO UPDATE SET estado = 'iniciada', centavos = $2, evento_en = $3`,
      [e.data.streamId, e.data.amount.amount, e.time],
    );
  }

  async aplicarCompraCobradaV1(e: Envelope<any>, posicion: number) {
    await this.#enUna(
      posicion,
      `UPDATE vista_conversion SET estado = 'cobrada', payment_id = $2, evento_en = $3
        WHERE stream_id = $1`,
      [e.data.streamId, e.data.paymentId, e.time],
    );
  }

  async aplicarCompraCompensadaV1(e: Envelope<any>, posicion: number) {
    await this.#enUna(
      posicion,
      `UPDATE vista_conversion SET estado = 'compensada', motivo = $2, evento_en = $3
        WHERE stream_id = $1`,
      [e.data.streamId, e.data.motivo, e.time],
    );
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
  #flujo: Flujo;
  #vista: Conversion;
  constructor(b: any, db: pg.Pool) {
    super(b);
    this.#diario = new Diario(db);
    this.#flujo = new Flujo(db);
    this.#vista = new Conversion(db);
    this.#acciones = acciones(new Clientes(transporte()));
  }

  get flujo() {
    return this.#flujo;
  }
  get vista() {
    return this.#vista;
  }

  /** Un evento al flujo, y de ahi a la vista.
   *
   *  La version esperada sale de LEER el flujo, no de un contador en memoria:
   *  con dos instancias, un contador local se desincroniza y el UNIQUE es lo
   *  unico que lo dice. */
  async #anotar<T>(streamId: string, tipo: string, data: T, causa: Envelope<unknown>) {
    const eventos = await this.#flujo.leer(streamId);
    const { version } = compraFold(reglasCompra, streamId, eventos);
    // El envelope se construye ANTES de escribirlo: con event sourcing el
    // flujo es el outbox, asi que el orden correcto es anotar primero y
    // publicar despues. Lo ideal seria que publicara un relay leyendo el
    // flujo —como hace `patterns.outbox`— y no esta linea: publicar aqui deja
    // una ventana en la que el evento esta anotado y nadie lo recibio.
    const e = newEnvelope(tipo, "checkout", data, causa);
    const nueva = await this.#flujo.append(streamId, version, e);
    await this.bus.publish(e);
    // La proyeccion se aplica aqui mismo. En un sistema mas grande la haria un
    // consumidor aparte, y el checkpoint es justo lo que permite separarlos sin
    // reprocesar todo.
    await conversionAplicar(this.#vista, e, nueva);
    return e;
  }

  get diario() {
    return this.#diario;
  }
  get pasos() {
    return this.#acciones;
  }

  async checkout(input: CheckoutIn, e: Envelope<unknown>): Promise<CheckoutOut> {
    // el id de la saga es el del flujo: el diario, el agregado y la traza
    // hablan del mismo hecho, y `axon trace` los cruza sin adivinar
    const stream = e.correlationId;
    await this.#anotar(stream, "compra.iniciada@v1",
      { streamId: stream, orderId: input.orderId, amount: input.amount }, e);
    const r = await correrCompra(stream, this.#acciones, this.#diario, e);
    if (r.estado === "completada") {
      const salidas = (await this.#diario.leer(stream))?.salidas ?? {};
      const paso1 = salidas[1] as { paymentId: string } | undefined;
      await this.#anotar(stream, "compra.cobrada@v1",
        { streamId: stream, paymentId: paso1?.paymentId ?? stream }, e);
    } else {
      await this.#anotar(stream, "compra.compensada@v1",
        { streamId: stream, motivo: String(r.error ?? "sin motivo") }, e);
    }
    return { estado: r.estado };
  }
}

if (process.env.NODE_TEST_CONTEXT === undefined) await main();

async function main() {
  arrancarTelemetria();
  const db = await esperarDb();
  const svc = new Checkout(bus(await conectar()), db);

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
