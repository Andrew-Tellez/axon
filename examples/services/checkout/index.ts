// El coordinador de la saga. Lo unico escrito a mano son las ENTRADAS de cada
// paso —datos de negocio— y el diario. El orden, la compensacion en orden
// inverso y el barrido los genero axon.
import { CheckoutService, Clients, httpRoutes, sweepRouteCompra, runCompra,
         sweepCompra, startSweepCompra, compraFold, compraLoad,
         compraSnapshot, conversionApply, pruneCompra, newEnvelope,
         pruneRouteCompra, rebuildConversion, rebuildRouteConversion,
         VersionConflict,
         type CheckoutIn, type CheckoutOut, type CompraActions, type CompraOutputs,
         type CompraRules, type ConversionProjection, type Checkpoint,
         type Envelope, type SnapshottingStream, type Outbox, type SagaJournal, type SagaStatus,
         type Shadow,
         type Transport } from "./contracts.ts";
import { arrancarTelemetria } from "../telemetria.ts";
import { bus, conectar, esperarDb, outbox, relay, servir } from "../runtime.ts";
import type pg from "pg";

/** HTTP contra los servicios declarados. El framework no elige transport: el
 *  cliente generado pone el timeout, los reintentos y el circuito encima. */
function transport(): Transport {
  return {
    async call(target, method, body, hdrs) {
      const routes: Record<string, string> = {
        capturePayment: "/v1/payments",
        refundPayment: `/v1/payments/${(body as any).paymentId}/refunds`,
        payoutMerchant: "/v1/payouts",
      };
      const r = await fetch(`http://${target}:8080${routes[method]}`, {
        method: "POST",
        headers: { "content-type": "application/json", ...hdrs },
        body: JSON.stringify(body),
      });
      if (!r.ok) throw new Error(`${target}.${method}: ${r.status} ${await r.text()}`);
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
class Journal implements SagaJournal {
  #db: pg.Pool;
  constructor(db: pg.Pool) {
    this.#db = db;
  }

  async open(id: string, _saga: string, e: Envelope<unknown>) {
    await this.#db.query(
      `INSERT INTO saga_compra (id, paso, estado, datos) VALUES ($1, 0, 'open', $2)
       ON CONFLICT (id) DO NOTHING`,
      [id, JSON.stringify(e)],
    );
  }

  async mark(id: string, step: number, status: string, output?: unknown) {
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
      [id, step, status, String(step), output === undefined ? null : JSON.stringify(output)],
    );
  }

  async close(id: string, status: SagaStatus) {
    await this.#db.query(
      `UPDATE saga_compra SET estado = $2, actualizado = now() WHERE id = $1`,
      [id, status],
    );
  }

  async read(id: string) {
    const { rows } = await this.#db.query(
      `SELECT paso, estado, coalesce(salidas, '{}'::jsonb) AS salidas
         FROM saga_compra WHERE id = $1 AND estado <> 'open'`,
      [id],
    );
    if (!rows[0]) return null;
    return {
      step: Number(rows[0].paso),
      status: rows[0].estado as string,
      outputs: rows[0].salidas as Record<number, unknown>,
    };
  }

  async claim(_saga: string, olderThan: Date, limit: number) {
    const { rows } = await this.#db.query(
      `UPDATE saga_compra SET actualizado = now()
        WHERE id IN (
          SELECT id FROM saga_compra
           WHERE estado IN ('open','attempting','done') AND actualizado < $1
           ORDER BY actualizado
           LIMIT $2
           FOR UPDATE SKIP LOCKED
        )
        RETURNING id, datos`,
      [olderThan, limit],
    );
    return rows.map((r: any) => ({ id: r.id as string, data: r.datos as Envelope<unknown> }));
  }
}

/** El flujo de eventos sobre Postgres.
 *
 *  `append` recibe la version que el llamador creia vigente. El UNIQUE
 *  (stream_id, version) es lo que la hace valer: si otro escribio en medio, la
 *  insercion falla y quien la recibe vuelve a leer. Sin el UNIQUE las dos
 *  entrarian y el estado reconstruido dependeria del orden de lectura. */
class Stream implements SnapshottingStream {
  #db: pg.Pool;
  #outbox: Outbox<pg.PoolClient>;
  constructor(db: pg.Pool, o: Outbox<pg.PoolClient>) {
    this.#db = db;
    this.#outbox = o;
  }

  /** Solo las fotos de la version de reglas vigente. Una de otra version se
   *  ignora y el estado se reconstruye entero: lento y correcto, en ese
   *  orden. */
  async snapshot(streamId: string, rules: number) {
    const { rows } = await this.#db.query(
      `SELECT version, estado FROM compra_snapshot
        WHERE stream_id = $1 AND reglas = $2 ORDER BY version DESC LIMIT 1`,
      [streamId, rules],
    );
    return rows[0] ? { version: Number(rows[0].version), state: rows[0].estado } : null;
  }

  async saveSnapshot(streamId: string, version: number, rules: number, state: unknown) {
    await this.#db.query(
      `INSERT INTO compra_snapshot (stream_id, version, reglas, estado)
       VALUES ($1,$2,$3,$4)
       ON CONFLICT (stream_id, version, reglas) DO NOTHING`,
      [streamId, version, rules, JSON.stringify(state)],
    );
  }

  /** Borra lo que la version vigente no usa: las fotos de otra version de
   *  reglas, y todas menos la mas nueva de cada flujo.
   *
   *  Una sola sentencia: en dos —primero las viejas, despues las de otra
   *  version— una limpieza interrumpida a la mitad deja un estado que nadie
   *  penso. Y puede ser agresiva porque una foto es una cache: lo peor que
   *  pasa es reconstruir desde el flujo. */
  async pruneSnapshots(rules: number) {
    const { rowCount } = await this.#db.query(
      `DELETE FROM compra_snapshot s
        WHERE s.reglas <> $1
           OR s.version < (SELECT max(version) FROM compra_snapshot
                            WHERE stream_id = s.stream_id AND reglas = $1)`,
      [rules],
    );
    return rowCount ?? 0;
  }

  async read(streamId: string, from = 0) {
    const { rows } = await this.#db.query(
      `SELECT version, type, data, en FROM compra_event
        WHERE stream_id = $1 AND version > $2 ORDER BY version`,
      [streamId, from],
    );
    return rows.map((r: any) => ({
      version: Number(r.version),
      type: r.type as string,
      data: r.data as unknown,
      // cuando OCURRIO. Al reconstruir la vista se vuelve a poner este, no la
      // hora de la reconstruccion.
      at: new Date(r.en).toISOString(),
    }));
  }

  /** Los flujos que existen, para poder recorrerlos al reconstruir. */
  async streams() {
    const { rows } = await this.#db.query(
      `SELECT DISTINCT stream_id FROM compra_event ORDER BY stream_id`,
    );
    return rows.map((r: any) => r.stream_id as string);
  }

  /** Anota el evento en el flujo Y lo deja listo para publicar, en UNA
   *  transaccion. Es lo que hace que no haya dual-write: el flujo es la verdad,
   *  el outbox es la entrega, y las dos filas entran o no entra ninguna.
   *
   *  El `stage` del outbox generado recibe esta transaccion, asi que la fila del
   *  outbox no se puede confirmar sin el evento. */
  async append(streamId: string, expected: number, e: Envelope<unknown>) {
    const version = expected + 1;
    const c = await this.#db.connect();
    try {
      await c.query("BEGIN");
      await c.query(
        `INSERT INTO compra_event (id, stream_id, version, type, data)
         VALUES ($1,$2,$3,$4,$5)`,
        [e.id, streamId, version, e.type, JSON.stringify(e.data)],
      );
      await this.#outbox.stage(e, c);
      await c.query("COMMIT");
    } catch (err: any) {
      await c.query("ROLLBACK").catch(() => {});
      // 23505: unique_violation. Otro escribio esta version primero, y eso no
      // es un error de programa: es la condicion normal de dos usuarios sobre
      // el mismo agregado.
      if (err?.code === "23505") throw new VersionConflict(streamId, expected);
      throw err;
    } finally {
      c.release();
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

const compraRules: CompraRules<Compra> = {
  initial: () => ({ estado: "nueva", centavos: 0, paymentId: null }),
  applyCompraIniciadaV1: (s, e) => ({ ...s, estado: "iniciada", centavos: e.amount.amount }),
  applyCompraCobradaV1: (s, e) => ({ ...s, estado: "cobrada", paymentId: e.paymentId }),
  applyCompraCompensadaV1: (s) => ({ ...s, estado: "compensada" }),
};

/** La proyeccion. Cada `aplicar*` guarda la posicion en la MISMA transaccion
 *  que su efecto: en dos, un corte entre ellas deja la vista adelantada o
 *  atrasada respecto de lo que dice haber aplicado, y ninguna da un error. */
class Conversion implements ConversionProjection, Checkpoint, Shadow {
  #db: pg.Pool;
  /** A que tabla escribe. La sombra es OTRA instancia, no un modo: un modo se
   *  queda encendido y la siguiente proyeccion en vivo escribe en la sombra
   *  sin que nada lo diga. */
  #tabla: string;
  #vista: string;
  constructor(db: pg.Pool, sombra = false) {
    this.#db = db;
    this.#tabla = sombra ? "vista_conversion_sombra" : "vista_conversion";
    this.#vista = sombra ? "conversion_sombra" : "conversion";
  }

  /** Deja la sombra vacia, con su punto en cero. */
  async prepare() {
    const c = await this.#db.connect();
    try {
      await c.query("BEGIN");
      await c.query(`DELETE FROM ${this.#tabla}`);
      await c.query(`DELETE FROM vista_conversion_checkpoint WHERE vista = $1`, [this.#vista]);
      await c.query("COMMIT");
    } catch (err) {
      await c.query("ROLLBACK").catch(() => {});
      throw err;
    } finally {
      c.release();
    }
  }

  /** Cambia la sombra por la viva, y su punto con ella, en UNA transaccion.
   *
   *  El intercambio son dos RENAME y el traspaso del punto. Postgres permite
   *  DDL transaccional, asi que quien lee espera unos milisegundos en vez de
   *  ver una vista a medias. En dos transacciones, un corte entre ellas deja la
   *  tabla nueva con el punto de la vieja: se saltaria eventos o los
   *  reprocesaria, y nada lo diria. */
  async swap() {
    const c = await this.#db.connect();
    try {
      await c.query("BEGIN");
      await c.query("ALTER TABLE vista_conversion RENAME TO vista_conversion_tmp");
      await c.query("ALTER TABLE vista_conversion_sombra RENAME TO vista_conversion");
      await c.query("ALTER TABLE vista_conversion_tmp RENAME TO vista_conversion_sombra");
      await c.query("DELETE FROM vista_conversion_checkpoint WHERE vista = 'conversion'");
      await c.query(
        `UPDATE vista_conversion_checkpoint SET vista = 'conversion'
          WHERE vista = 'conversion_sombra'`,
      );
      await c.query("COMMIT");
    } catch (err) {
      await c.query("ROLLBACK").catch(() => {});
      throw err;
    } finally {
      c.release();
    }
  }

  async read(view: string, streamId: string) {
    const { rows } = await this.#db.query(
      `SELECT posicion FROM vista_conversion_checkpoint
        WHERE vista = $1 AND stream_id = $2`,
      [view, streamId],
    );
    return rows[0] ? Number(rows[0].posicion) : 0;
  }

  /** El efecto de la vista y su punto, en la MISMA transaccion. En dos, un
   *  corte entre ellas deja la vista adelantada o atrasada respecto de lo que
   *  dice haber aplicado, y ninguna de las dos cosas da un error. */
  async #enUna(streamId: string, posicion: number, sql: string, args: unknown[]) {
    const c = await this.#db.connect();
    try {
      await c.query("BEGIN");
      await c.query(sql, args);
      await c.query(
        `INSERT INTO vista_conversion_checkpoint (vista, stream_id, posicion)
         VALUES ('${this.#vista}', $1, $2)
         ON CONFLICT (vista, stream_id)
         DO UPDATE SET posicion = GREATEST(vista_conversion_checkpoint.posicion, $2)`,
        [streamId, posicion],
      );
      await c.query("COMMIT");
    } catch (err) {
      await c.query("ROLLBACK").catch(() => {});
      throw err;
    } finally {
      c.release();
    }
  }

  async applyCompraIniciadaV1(e: Envelope<any>, posicion: number) {
    // Interruptor del demo: alarga la reconstruccion para poder MEDIR la
    // ventana. Sin esto termina en milisegundos y "nadie vio nada" no se
    // distingue de "nadie mirO".
    const lento = Number(process.env.AXON_DEMO_RECONSTRUIR_LENTO_MS ?? 0);
    if (lento > 0 && e.source === "rebuild") {
      await new Promise((r) => setTimeout(r, lento));
    }
    await this.#enUna(
      e.data.streamId,
      posicion,
      `INSERT INTO ${this.#tabla} (stream_id, estado, centavos, evento_en)
       VALUES ($1,'iniciada',$2,$3)
       ON CONFLICT (stream_id) DO UPDATE SET estado = 'iniciada', centavos = $2, evento_en = $3`,
      [e.data.streamId, e.data.amount.amount, e.time],
    );
  }

  async applyCompraCobradaV1(e: Envelope<any>, posicion: number) {
    await this.#enUna(
      e.data.streamId,
      posicion,
      `UPDATE ${this.#tabla} SET estado = 'cobrada', payment_id = $2, evento_en = $3
        WHERE stream_id = $1`,
      [e.data.streamId, e.data.paymentId, e.time],
    );
  }

  async applyCompraCompensadaV1(e: Envelope<any>, posicion: number) {
    await this.#enUna(
      e.data.streamId,
      posicion,
      `UPDATE ${this.#tabla} SET estado = 'compensada', motivo = $2, evento_en = $3
        WHERE stream_id = $1`,
      [e.data.streamId, e.data.motivo, e.time],
    );
  }
}

/** Las entradas de cada paso: lo unico que el generador no puede saber. El
 *  orden lo sabe el coordinador; el contenido, no.
 *
 *  Todo sale del envelope y de `prior`, nunca de una variable de este
 *  proceso: el barrido corre estas mismas acciones sobre una saga que arranco
 *  en OTRO proceso, y ahi una closure no existe. */
function actions(clients: Clients): CompraActions {
  const entrada = (e: Envelope<unknown>) => e.data as CheckoutIn;
  return {
    async step1CapturePayment(e) {
      const { orderId, amount } = entrada(e);
      return clients.paymentsCapturePayment({ orderId, amount }, e);
    },
    async undo1RefundPayment(e, prior) {
      // sin cobro no hay nada que deshacer, y eso no es un error: el
      // coordinador compensa tambien el paso que quedo en duda
      if (!prior.step1) return;
      await clients.paymentsRefundPayment({ paymentId: prior.step1.paymentId }, e);
    },
    async step2PayoutMerchant(e, prior) {
      if (!prior.step1) throw new Error("no hay cobro que pagar");
      return clients.paymentsPayoutMerchant(
        { paymentId: prior.step1.paymentId, amount: entrada(e).amount },
        e,
      );
    },
  };
}

class Checkout extends CheckoutService {
  #journal: SagaJournal;
  #actions: CompraActions;
  #stream: Stream;
  #vista: Conversion;
  #sombra: Conversion;
  constructor(b: any, o: Outbox<pg.PoolClient>, db: pg.Pool) {
    super(b, o);
    this.#journal = new Journal(db);
    this.#stream = new Stream(db, o);
    this.#vista = new Conversion(db);
    this.#sombra = new Conversion(db, true);
    this.#actions = actions(new Clients(transport()));
  }

  get stream() {
    return this.#stream;
  }
  get vista() {
    return this.#vista;
  }
  get sombra() {
    return this.#sombra;
  }

  /** Un evento al flujo, y de ahi a la vista.
   *
   *  La version esperada sale de LEER el flujo, no de un contador en memoria:
   *  con dos instancias, un contador local se desincroniza y el UNIQUE es lo
   *  unico que lo dice. */
  async #anotar<T>(streamId: string, tipo: string, data: T, causa: Envelope<unknown>) {
    // `compraLoad` lee la ultima foto valida y solo el resto del flujo desde
    // ahi. Sin fotos leeria el flujo entero, que es lo mismo hasta que un flujo
    // tiene cien mil eventos.
    const { version } = await compraLoad(compraRules, this.#stream, streamId);
    // El envelope se construye ANTES de escribirlo. Nadie publica aqui: el
    // append deja la fila en el outbox en la misma transaccion, y de ahi
    // publica el relay. Un `publish` en esta linea seria el dual-write que
    // todo esto evita.
    const e = newEnvelope(tipo, "checkout", data, causa);
    const nueva = await this.#stream.append(streamId, version, e);
    // La foto se saca DESPUES del append y de un estado recien reconstruido:
    // fotografiar lo que se creia el estado antes de escribir guardaria una
    // foto de algo que no quedo en el flujo.
    const { state } = await compraLoad(compraRules, this.#stream, streamId);
    await compraSnapshot(this.#stream, streamId, nueva, state);
    // La proyeccion se aplica aqui mismo. En un sistema mas grande la haria un
    // consumidor aparte, y el checkpoint es justo lo que permite separarlos sin
    // reprocesar todo.
    await conversionApply(this.#vista, e, nueva);
    return e;
  }

  get journal() {
    return this.#journal;
  }
  get steps() {
    return this.#actions;
  }

  async checkout(input: CheckoutIn, e: Envelope<unknown>): Promise<CheckoutOut> {
    // el id de la saga es el del flujo: el diario, el agregado y la traza
    // hablan del mismo hecho, y `axon trace` los cruza sin adivinar
    const stream = e.correlationId;
    await this.#anotar(stream, "compra.iniciada@v1",
      { streamId: stream, orderId: input.orderId, amount: input.amount }, e);
    const r = await runCompra(stream, this.#actions, this.#journal, e);
    if (r.status === "completed") {
      const outputs = (await this.#journal.read(stream))?.outputs ?? {};
      const step1 = outputs[1] as { paymentId: string } | undefined;
      await this.#anotar(stream, "compra.cobrada@v1",
        { streamId: stream, paymentId: step1?.paymentId ?? stream }, e);
    } else {
      await this.#anotar(stream, "compra.compensada@v1",
        { streamId: stream, motivo: String(r.error ?? "sin motivo") }, e);
    }
    return { estado: r.status };
  }
}

if (process.env.NODE_TEST_CONTEXT === undefined) await main();

async function main() {
  arrancarTelemetria();
  const db = await esperarDb();
  const b = bus(await conectar());
  const svc = new Checkout(b, outbox(), db);

  // El relay: lo unico que publica. Sin el, los eventos quedan anotados en el
  // flujo y en el outbox, y nadie los recibe.
  relay(db, b);

  // Dentro del proceso Y por la ruta que golpea el programador que despliega
  // `axon infra`: un servicio escalado a cero no barre nada por su cuenta.
  startSweepCompra(svc.steps, svc.journal, (r) => {
    if (r.claimed) console.log(`[checkout] barrido ${JSON.stringify(r)}`);
    if (r.stuck) console.error(`[checkout] ${r.stuck} sagas ATASCADAS`);
  });

  servir(
    Number(process.env.PORT ?? 8080),
    {
      "POST /v1/checkouts": (body, e) => svc.checkout(body, e),
      [sweepRouteCompra]: () => sweepCompra(svc.steps, svc.journal),
      // la ruta que golpea el cron que despliega `axon infra`
      [pruneRouteCompra]: async () => ({ borradas: await pruneCompra(svc.stream) }),
      // Reconstruir no lleva cron: no es periodico, es una operacion que
      // alguien decide.
      [rebuildRouteConversion]: async () => ({
        aplicados: await rebuildConversion(svc.sombra, svc.stream),
      }),
    },
    httpRoutes,
  );
}
