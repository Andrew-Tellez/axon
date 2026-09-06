// generado por axon desde checkout.toml — no editar


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

export interface CompraIniciadaV1 {
  streamId: string;
  orderId: string;
  amount: { amount: number; currency: string };
}

export interface CompraCobradaV1 {
  streamId: string;
  paymentId: string;
}

export interface CompraCompensadaV1 {
  streamId: string;
  motivo: string;
}

export interface CheckoutIn {
  orderId: string;
  amount: { amount: number; currency: string };
}

export interface CheckoutOut {
  estado: string;
}

export const manifest = {
  "service": "checkout",
  "version": "1.0.0",
  "owner": "equipo-comercio",
  "tier": "1",
  "pii": [],
  "external": false,
  "emits": {
    "compra.iniciada@v1": {
      "streamId": "uuid",
      "orderId": "uuid",
      "amount": "money"
    },
    "compra.cobrada@v1": {
      "streamId": "uuid",
      "paymentId": "uuid"
    },
    "compra.compensada@v1": {
      "streamId": "uuid",
      "motivo": "string"
    }
  },
  "consumes": {},
  "methods": {
    "checkout": {
      "in": {
        "orderId": "uuid",
        "amount": "money"
      },
      "out": {
        "estado": "string"
      },
      "http": "POST /v1/checkouts",
      "idempotent": true,
      "auth": "required",
      "rate_limit": null,
      "timeout_ms": 30000,
      "paginated": false
    }
  },
  "depends": [
    {
      "service": "payments",
      "external": null,
      "method": "capturePayment",
      "via": null,
      "timeout_ms": 8000,
      "retries": 0,
      "breaker": true
    },
    {
      "service": "payments",
      "external": null,
      "method": "refundPayment",
      "via": null,
      "timeout_ms": 8000,
      "retries": 3,
      "breaker": true
    },
    {
      "service": "payments",
      "external": null,
      "method": "payoutMerchant",
      "via": null,
      "timeout_ms": 4000,
      "retries": 2,
      "breaker": true
    }
  ],
  "patterns": {
    "outbox": true
  },
  "cap": {
    "consistency": "eventual",
    "on_partition": "reject",
    "max_staleness_ms": 5000
  },
  "flags": {},
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
  "machine": {},
  "saga": {
    "compra": {
      "on": "checkout",
      "steps": [
        {
          "do": "payments.capturePayment",
          "undo": "payments.refundPayment"
        },
        {
          "do": "payments.payoutMerchant",
          "undo": null
        }
      ],
      "timeout_ms": 60000
    }
  },
  "aggregate": {
    "compra": {
      "events": [
        "compra.iniciada@v1",
        "compra.cobrada@v1",
        "compra.compensada@v1"
      ],
      "machine": null,
      "snapshot_every": 2,
      "snapshot_version": 1
    }
  },
  "view": {
    "conversion": {
      "on": [
        "compra.iniciada@v1",
        "compra.cobrada@v1",
        "compra.compensada@v1"
      ],
      "table": null,
      "max_staleness_ms": 3000
    }
  },
  "infra": {
    "state": "postgres",
    "runtime": "container",
    "migrations": "sql/checkout/",
    "secrets": [],
    "min_instances": 1,
    "max_instances": null,
    "port": 8080,
    "buckets": {},
    "pool_size": 4,
    "max_connections": 100,
    "ha": null,
    "backup_retention_days": 7,
    "pitr": null,
    "read_replicas": null,
    "shard_key": null,
    "tenant_column": null,
    "tenant_exempt": []
  },
  "env": {
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

export abstract class CheckoutService {
  protected readonly bus: Bus;
  protected readonly outbox: Outbox;
  constructor(bus: Bus, outbox: Outbox) {
    this.bus = bus;
    this.outbox = outbox;
  }
  static readonly wellKnown = "/.well-known/axon.json";
  protected emitCompraIniciadaV1(data: CompraIniciadaV1, tx: unknown, cause?: Envelope<unknown>) {
    return this.outbox.stage(newEnvelope("compra.iniciada@v1", "checkout", data, cause), tx);
  }
  protected emitCompraCobradaV1(data: CompraCobradaV1, tx: unknown, cause?: Envelope<unknown>) {
    return this.outbox.stage(newEnvelope("compra.cobrada@v1", "checkout", data, cause), tx);
  }
  protected emitCompraCompensadaV1(data: CompraCompensadaV1, tx: unknown, cause?: Envelope<unknown>) {
    return this.outbox.stage(newEnvelope("compra.compensada@v1", "checkout", data, cause), tx);
  }
  abstract checkout(input: CheckoutIn, e: Envelope<unknown>): Promise<CheckoutOut>;
}


/** El diario de una saga: donde vive el avance. Sin el, un reinicio a
 *  mitad de camino deja los pasos ya hechos aplicados y sin registro de
 *  cuales fueron: no se puede terminar ni compensar.
 *
 *  `intentando` se escribe ANTES de la llamada y `hecho` DESPUES. Un paso
 *  que quedo en `intentando` puede haber ocurrido o no, asi que al retomar
 *  se COMPENSA, no se reintenta: por eso toda compensacion tiene que
 *  tolerar que no haya nada que deshacer.
 *
 *  Guarda tambien el envelope que la arranco. Retomar sin el es imposible:
 *  las acciones necesitan los datos de la llamada, y el proceso que los
 *  tenia en memoria es justo el que se murio. */
export interface SagaDiario {
  abrir(id: string, saga: string, e: Envelope<unknown>): Promise<void>;
  marcar(id: string, paso: number, estado: "intentando" | "hecho" | "deshecho", salida?: unknown): Promise<void>;
  cerrar(id: string, estado: SagaEstado): Promise<void>;
  /** Hasta donde llego, y lo que devolvio cada paso. `null` si es nueva.
   *
   *  Las salidas hacen falta para COMPENSAR: deshacer un paso suele
   *  necesitar el id que ESE paso devolvio, y tras un reinicio ese valor
   *  no esta en ninguna variable —el proceso que lo tenia es justo el
   *  que se murio—. Guardarlo en el diario es lo que mantiene la
   *  compensacion posible. */
  leer(id: string): Promise<{ paso: number; estado: string; salidas: Record<number, unknown> } | null>;
  /** Reclama hasta `limite` sagas abiertas que no avanzan desde `antesDe`,
   *  y devuelve el envelope de cada una.
   *
   *  RECLAMA, no lista: dos instancias del servicio barren a la vez, y dos
   *  coordinadores sobre la misma saga compensan los mismos pasos dos
   *  veces. En Postgres el reclamo y el filtro son la misma sentencia:
   *
   *    UPDATE saga_<nombre> SET actualizado = now()
   *     WHERE estado IN ('intentando','hecho') AND actualizado < $1
   *     RETURNING id, datos
   *    LIMIT $2
   *
   *  Tocar `actualizado` es el reclamo: el otro barredor ya no la ve. Y si
   *  este proceso muere a mitad, vuelve a ser elegible en la siguiente
   *  ventana sin que nadie la desbloquee a mano. */
  reclamar(saga: string, antesDe: Date, limite: number): Promise<{ id: string; datos: Envelope<unknown> }[]>;
}

export type SagaEstado = "completada" | "compensada" | "atascada";

/** Una compensacion que falla no tiene nada detras: la saga queda a
 *  medias y necesita una persona. Se lanza para que eso no pase
 *  desapercibido. */
export class SagaAtascada extends Error {
  readonly saga: string;
  readonly paso: number;
  readonly causa: unknown;
  constructor(saga: string, paso: number, causa: unknown) {
    super(`${saga}: la compensacion del paso ${paso} fallo; la saga quedo a medias`);
    this.saga = saga;
    this.paso = paso;
    this.causa = causa;
  }
}

/** Lo que hizo una pasada del barrido. Se devuelve para que se pueda
 *  medir: un barrido que no reporta nada es indistinguible de uno que no
 *  corre. */
export interface SagaBarrido {
  reclamadas: number;
  completadas: number;
  compensadas: number;
  /** Necesitan una persona. El barrido NO las reintenta. */
  atascadas: number;
  /** Quedaron para la proxima pasada porque se alcanzo el limite. */
  pendientes: boolean;
}

/** La ruta que golpea el programador para correr una pasada del
 *  barrido. `axon infra` la despliega en los cuatro targets, asi que el
 *  arranque tiene que servirla llamando a `barrerCompra`: un programador
 *  apuntando a un 404 se aplica sin error y no barre nada.
 *
 *  NO es un metodo declarado, asi que no sale por el gateway. Dispara
 *  compensaciones: no puede ser publica. */
export const rutaBarridoCompra = "POST /internal/saga/compra/barrer" as const;

/** Los pasos declarados en el manifiesto. Generado: no editar. */
export const compraPasos = [
  { paso: 1, hacer: "payments.capturePayment", deshacer: "payments.refundPayment" },
  // el ultimo paso no lleva compensacion: si falla, no hay nada suyo que deshacer
  { paso: 2, hacer: "payments.payoutMerchant", deshacer: null },
] as const;

/** Lo que devolvio cada paso, guardado en el diario. Es lo que hace
 *  posible compensar despues de un reinicio: deshacer un paso suele
 *  necesitar el id que ese paso devolvio, y una variable en memoria no
 *  sobrevive al proceso que la tenia. */
export interface CompraSalidas {
  paso1?: PaymentsCapturePaymentOut;
  paso2?: PaymentsPayoutMerchantOut;
}

/** Un metodo por paso y uno por compensacion. Los implementa quien
 *  conoce los datos: el coordinador sabe el orden, no el contenido. */
export interface CompraAcciones {
  /** paso 1 · payments.capturePayment */
  paso1CapturePayment(e: Envelope<unknown>, previas: CompraSalidas): Promise<PaymentsCapturePaymentOut>;
  /** deshace el paso 1 · payments.refundPayment · recibe lo que devolvieron los pasos
   *  anteriores, y tiene que tolerar que no haya nada que deshacer */
  deshacer1RefundPayment(e: Envelope<unknown>, previas: CompraSalidas): Promise<void>;
  /** paso 2 · payments.payoutMerchant */
  paso2PayoutMerchant(e: Envelope<unknown>, previas: CompraSalidas): Promise<PaymentsPayoutMerchantOut>;
}

/** Corre la saga `compra`.
 *
 *  Hacia adelante hasta que un paso falla o se agota el presupuesto; de
 *  ahi en orden INVERSO deshaciendo solo lo que se intento. El orden
 *  inverso no es estetica: compensar hacia adelante deshace un paso
 *  cuyo efecto otro paso posterior ya uso.
 *
 *  Si `id` ya tiene diario, retoma: el paso que quedo en `intentando`
 *  se compensa, porque no se sabe si ocurrio. */
export async function correrCompra(
  id: string,
  acciones: CompraAcciones,
  diario: SagaDiario,
  e: Envelope<unknown>,
): Promise<{ estado: SagaEstado; hasta: number; error?: unknown }> {
  const total = 2;
  // presupuesto declarado en el manifiesto; `axon verify` ya comprobo
  // que cubre la suma de los pasos y sus compensaciones
  const limite = Date.now() + 60000;
  const previo = await diario.leer(id);
  if (!previo) await diario.abrir(id, "compra", e);
  // Rehidratado del diario, no de una variable: al retomar, esto es lo
  // unico que queda de lo que hicieron los pasos anteriores.
  //
  // El diario las guarda por NUMERO de paso y aca se nombran: un cast
  // de una forma a la otra compila y deja todo en `undefined`, asi que
  // la traduccion es explicita, campo por campo.
  const guardadas = previo?.salidas ?? {};
  const previas: CompraSalidas = {
    paso1: guardadas[1] as PaymentsCapturePaymentOut | undefined,
    paso2: guardadas[2] as PaymentsPayoutMerchantOut | undefined,
  };
  // un paso a medio intentar no se reintenta: se deshace
  let hecho = previo ? (previo.estado === "hecho" ? previo.paso : previo.paso - 1) : 0;
  const dudoso = previo?.estado === "intentando" ? previo.paso : 0;
  // El paso que FALLO tambien se deshace: un timeout no dice que no
  // paso nada del otro lado. Compensar solo hasta el ultimo exito
  // deja ese efecto aplicado para siempre.
  let intentado = dudoso;
  let fallo: unknown = dudoso ? new Error("retomada con un paso en duda") : undefined;
  if (!dudoso) {
    for (let paso = hecho + 1; paso <= total; paso++) {
      if (Date.now() > limite) {
        fallo = new Error(`compra: presupuesto agotado antes del paso ${paso}`);
        break;
      }
      intentado = paso;
      try {
        await pasoCompra(paso, acciones, diario, e, id, previas);
        hecho = paso;
      } catch (err) {
        fallo = err;
        break;
      }
    }
  }
  if (!fallo) {
    await diario.cerrar(id, "completada");
    return { estado: "completada", hasta: total };
  }
  // de vuelta: todo lo que se INTENTO, en orden inverso
  for (let paso = intentado; paso >= 1; paso--) {
    try {
      await deshacerCompra(paso, acciones, e, previas);
      await diario.marcar(id, paso, "deshecho");
    } catch (err) {
      await diario.cerrar(id, "atascada");
      throw new SagaAtascada("compra", paso, err);
    }
  }
  await diario.cerrar(id, "compensada");
  return { estado: "compensada", hasta: hecho, error: fallo };
}

async function pasoCompra(
  paso: number,
  acciones: CompraAcciones,
  diario: SagaDiario,
  e: Envelope<unknown>,
  id: string,
  previas: CompraSalidas,
): Promise<void> {
  switch (paso) {
      case 1:
        await diario.marcar(id, 1, "intentando");
        previas.paso1 = await acciones.paso1CapturePayment(e, previas);
        // la salida se guarda CON el `hecho`: en dos escrituras, un
        // corte entre las dos deja el paso hecho y su resultado perdido
        await diario.marcar(id, 1, "hecho", previas.paso1);
        break;
      case 2:
        await diario.marcar(id, 2, "intentando");
        previas.paso2 = await acciones.paso2PayoutMerchant(e, previas);
        // la salida se guarda CON el `hecho`: en dos escrituras, un
        // corte entre las dos deja el paso hecho y su resultado perdido
        await diario.marcar(id, 2, "hecho", previas.paso2);
        break;
    default:
      throw new Error(`compra: paso ${paso} no declarado en el manifiesto`);
  }
}

async function deshacerCompra(paso: number, acciones: CompraAcciones, e: Envelope<unknown>, previas: CompraSalidas): Promise<void> {
  switch (paso) {
      case 1:
        await acciones.deshacer1RefundPayment(e, previas);
        break;
      case 2:
        break; // sin compensacion declarada: es el ultimo paso
    default:
      throw new Error(`compra: paso ${paso} no declarado en el manifiesto`);
  }
}

/** Una pasada del barrido: retoma las sagas `compra` que no avanzan.
 *
 *  Solo toca las que llevan mas de su PRESUPUESTO sin moverse (60000ms).
 *  Ese umbral no es una heuristica: `axon verify` ya comprobo que el
 *  presupuesto cubre la suma de los pasos y sus compensaciones, asi que
 *  una saga mas vieja que eso no esta en camino, esta colgada. Barrer
 *  antes seria correr un segundo coordinador sobre una saga viva.
 *
 *  Una que quedo `atascada` NO se reintenta: se cuenta y se deja. Una
 *  compensacion que ya fallo necesita una persona, y reintentarla en
 *  silencio esconde justo eso. */
export async function barrerCompra(
  acciones: CompraAcciones,
  diario: SagaDiario,
  limite = 50,
): Promise<SagaBarrido> {
  const antesDe = new Date(Date.now() - 60000);
  const colgadas = await diario.reclamar("compra", antesDe, limite);
  const r: SagaBarrido = {
    reclamadas: colgadas.length,
    completadas: 0,
    compensadas: 0,
    atascadas: 0,
    // si se lleno el limite hay mas esperando, y decirlo es la
    // diferencia entre un barrido que va al dia y uno que no alcanza
    pendientes: colgadas.length >= limite,
  };
  for (const { id, datos } of colgadas) {
    try {
      const salida = await correrCompra(id, acciones, diario, datos);
      if (salida.estado === "completada") r.completadas++;
      else r.compensadas++;
    } catch (err) {
      // una saga atascada no aborta la pasada: las demas siguen
      // colgadas y este es el unico que las va a mirar
      if (err instanceof SagaAtascada) r.atascadas++;
      else throw err;
    }
  }
  return r;
}

/** Arranca el barrido periodico y devuelve como pararlo.
 *
 *  El intervalo sale del presupuesto declarado: nada se vuelve elegible
 *  antes, asi que barrer mas seguido es trabajo sin resultado. Es seguro
 *  con varias instancias porque `reclamar` reclama.
 *
 *  `alTerminar` recibe cada pasada. Conectalo a las metricas: un barrido
 *  que no reporta es indistinguible de uno que no corre, y este es el
 *  unico lugar desde donde se ve que una saga quedo atascada. */
export function arrancarBarridoCompra(
  acciones: CompraAcciones,
  diario: SagaDiario,
  alTerminar: (r: SagaBarrido) => void,
  intervaloMs = 60000,
): () => void {
  let corriendo = false;
  const t = setInterval(async () => {
    // sin esto, una pasada lenta se solapa con la siguiente en el
    // mismo proceso y las dos reclaman
    if (corriendo) return;
    corriendo = true;
    try {
      alTerminar(await barrerCompra(acciones, diario));
    } finally {
      corriendo = false;
    }
  }, intervaloMs);
  // que un barrido de fondo no mantenga el proceso vivo al apagarlo
  t.unref?.();
  return () => clearInterval(t);
}


/** El flujo de un agregado. Es la fuente de verdad: lo que hoy se
 *  guarda en una fila es una PROYECCION de esto.
 *
 *  `append` recibe la version que el llamador creia vigente. Si otro
 *  escribio en medio, tiene que fallar: eso es el UNIQUE (stream_id,
 *  version) haciendo su trabajo, y `axon verify` comprueba que exista
 *  porque sin el las dos escrituras entran y nadie ve un error. */
/** Un evento tal como esta en el flujo. `en` es CUANDO OCURRIO, y viaja
 *  porque al reconstruir una vista hay que volver a ponerlo: rellenarlo con
 *  la hora de la reconstruccion reescribe el historial en silencio. */
export interface EventoDelFlujo {
  version: number;
  type: string;
  data: unknown;
  en: string;
}

export interface FlujoEventos {
  /** Los eventos de un flujo, en orden. `desde` permite arrancar de una foto. */
  leer(streamId: string, desde?: number): Promise<EventoDelFlujo[]>;
  /** Agrega al final. Rechaza si `esperada` ya no es la ultima version. */
  append(streamId: string, esperada: number, e: Envelope<unknown>): Promise<number>;
  /** Las fotos van en `FlujoConFotos`, que solo se genera si el
   *  manifiesto las declara: opcionales aqui, declararlas y no
   *  implementarlas compilaria y no haria nada. */
}

/** Otro escribio primero. No es un error de programa: es la condicion
 *  normal de dos usuarios sobre el mismo agregado, y quien la recibe
 *  vuelve a leer y reintenta. */
export class VersionEnConflicto extends Error {
  readonly streamId: string;
  readonly esperada: number;
  constructor(streamId: string, esperada: number) {
    super(`${streamId}: la version ${esperada} ya no es la ultima`);
    this.streamId = streamId;
    this.esperada = esperada;
  }
}

/** Un flujo que ademas guarda fotos.
 *
 *  Una foto es una CACHE del `fold`, y por eso lleva la version de las
 *  reglas con la que se calculo: si el `fold` cambia, las fotos viejas
 *  codifican la version anterior y rehidratar de ahi da un estado que
 *  ya no coincide con reproducir el flujo. Eso no da ningun error: da
 *  un numero equivocado.
 *
 *  `foto` devuelve solo las de la version vigente. Las de otra version
 *  se ignoran y el estado se reconstruye desde el principio, que es
 *  lento y correcto —en ese orden. */
export interface FlujoConFotos extends FlujoEventos {
  foto(streamId: string, reglas: number): Promise<{ version: number; estado: unknown } | null>;
  guardarFoto(streamId: string, version: number, reglas: number, estado: unknown): Promise<void>;
  /** Borra las fotos que la version vigente no usa: las de OTRA version
   *  de reglas, y todas menos la mas nueva de cada flujo. Devuelve
   *  cuantas borro.
   *
   *  Puede ser agresiva justamente porque una foto es una CACHE: lo
   *  peor que puede pasar es reconstruir desde el flujo, que es lento y
   *  correcto. Borrar de mas no rompe nada; no borrar nunca hace crecer
   *  la tabla con cada version de reglas.
   *
   *  Y no hay carrera con quien rehidrata: `foto` devuelve el estado por
   *  valor, asi que borrar la fila despues no le quita nada. */
  limpiarFotos(reglas: number): Promise<number>;
}

/** Los eventos que componen `compra`, declarados en el manifiesto. */
export const compraEventos = ["compra.iniciada@v1", "compra.cobrada@v1", "compra.compensada@v1"] as const;
export type CompraEvento = typeof compraEventos[number];

/** El estado reconstruido. Su forma la define el dominio; lo que
 *  impone el generador es que haya un caso por cada evento declarado. */
export interface CompraReglas<CompraEstado> {
  /** El estado antes del primer evento. */
  inicial(streamId: string): CompraEstado;
  aplicarCompraIniciadaV1(estado: CompraEstado, e: CompraIniciadaV1): CompraEstado;
  aplicarCompraCobradaV1(estado: CompraEstado, e: CompraCobradaV1): CompraEstado;
  aplicarCompraCompensadaV1(estado: CompraEstado, e: CompraCompensadaV1): CompraEstado;
}

/** Reconstruye el estado aplicando el flujo en orden. */
export function compraFold<E>(
  reglas: CompraReglas<E>,
  streamId: string,
  eventos: { version: number; type: string; data: unknown }[],
  desde?: { version: number; estado: E },
): { version: number; estado: E } {
  let estado = desde ? desde.estado : reglas.inicial(streamId);
  let version = desde ? desde.version : 0;
  for (const ev of eventos) {
    // El orden no se asume: un hueco en las versiones significa que
    // falta un evento, y reconstruir sin el da un estado que nunca
    // existio.
    if (ev.version !== version + 1) {
      throw new Error(`compra/${streamId}: se esperaba la version ${version + 1} y llego ${ev.version}`);
    }
    estado = compraAplicar(reglas, estado, ev);
    version = ev.version;
  }
  return { version, estado };
}

function compraAplicar<E>(reglas: CompraReglas<E>, estado: E, ev: { type: string; data: unknown }): E {
  switch (ev.type) {
      case "compra.iniciada@v1":
        return reglas.aplicarCompraIniciadaV1(estado, ev.data as CompraIniciadaV1);
      case "compra.cobrada@v1":
        return reglas.aplicarCompraCobradaV1(estado, ev.data as CompraCobradaV1);
      case "compra.compensada@v1":
        return reglas.aplicarCompraCompensadaV1(estado, ev.data as CompraCompensadaV1);
    default:
      // Un evento en el flujo que el manifiesto no declara: el estado
      // que saldria de ignorarlo es incorrecto y nadie lo sabria.
      throw new Error(`compra: `+ev.type+` no es un evento declarado del agregado`);
  }
}

/** Cada cuantos eventos se fotografia, y con que version de reglas.
 *  Los dos numeros salen del manifiesto: nadie los teclea dos veces. */
export const compraFotoCada = 2;
export const compraFotoReglas = 1;

/** Carga el estado: de la ultima foto valida, y solo el resto del
 *  flujo desde ahi.
 *
 *  Si no hay foto de la version vigente, reconstruye entero. Eso es
 *  lento y correcto, en ese orden: una foto de otra version daria un
 *  estado incorrecto sin decirlo. */
export async function compraCargar<E>(
  reglas: CompraReglas<E>,
  flujo: FlujoConFotos,
  streamId: string,
): Promise<{ version: number; estado: E }> {
  const f = await flujo.foto(streamId, compraFotoReglas);
  const desde = f ? { version: f.version, estado: f.estado as E } : undefined;
  const eventos = await flujo.leer(streamId, f?.version ?? 0);
  return compraFold(reglas, streamId, eventos, desde);
}

/** Fotografia si toca. Devuelve si la guardo, para poder medirlo:
 *  una cadencia declarada que no se cumple es una foto que nadie
 *  sabe que falta. */
export async function compraFotografiar<E>(
  flujo: FlujoConFotos,
  streamId: string,
  version: number,
  estado: E,
): Promise<boolean> {
  if (version === 0 || version % compraFotoCada !== 0) return false;
  await flujo.guardarFoto(streamId, version, compraFotoReglas, estado);
  return true;
}

/** La ruta que golpea el programador para limpiar las fotos viejas.
 *  `axon infra` la despliega en los cuatro targets. */
export const rutaLimpiezaCompra = "POST /internal/aggregate/compra/limpiar" as const;

/** Una pasada de limpieza. Devuelve cuantas fotos borro, para poder
 *  medirlo: una limpieza que no reporta nada es indistinguible de una
 *  que no corre, y lo que se nota entonces es el tamano de la tabla. */
export async function limpiarCompra(flujo: FlujoConFotos): Promise<number> {
  return flujo.limpiarFotos(compraFotoReglas);
}


/** Donde una vista anota hasta donde llego.
 *
 *  Sin esto, un reinicio reprocesa desde el principio o se salta lo que no
 *  alcanzo a aplicar. Las dos cosas dan una vista incorrecta, y ninguna da
 *  un error: por eso `axon verify` exige la tabla. */
export interface Checkpoint {
  /** Desde donde retomar. La ESCRITURA no esta aqui a proposito: la
   *  posicion tiene que guardarse en la misma transaccion que el efecto
   *  de la vista, y esa transaccion es de la proyeccion, no del
   *  framework. En dos transacciones, un corte entre ellas deja la vista
   *  adelantada o atrasada respecto de lo que dice haber aplicado, y
   *  ninguna de las dos da un error.
   *
   *  Por eso cada `aplicar*` recibe la posicion: la guarda quien puede
   *  hacerlo junto con el resto.
   *
   *  Y es POR FLUJO. La version de un evento es su posicion dentro de su
   *  flujo, asi que un solo numero para toda la vista no identifica nada
   *  en cuanto hay mas de un flujo: con uno parecia funcionar. */
  leer(vista: string, streamId: string): Promise<number>;
}

/** Una vista sombra: se construye aparte y se cambia por la viva de
 *  golpe.
 *
 *  Reconstruir en el sitio deja la vista incompleta mientras corre, y se
 *  sigue leyendo: los que preguntan reciben menos filas de las que hay,
 *  sin error. Con sombra, nadie ve un estado intermedio.
 *
 *  La proyeccion que se le pasa a `reconstruir` tiene que estar apuntada a
 *  la SOMBRA. Si apuntara a la viva, esto seria una reconstruccion en el
 *  sitio con pasos extra —y por eso el demo mide que las lecturas nunca
 *  bajen mientras corre. */
export interface Sombra {
  /** Deja la sombra vacia, con su punto en cero. */
  preparar(): Promise<void>;
  /** Cambia la sombra por la viva, y su punto con ella, en UNA
   *  transaccion. En dos, un corte entre ellas deja la vista nueva con el
   *  punto de la vieja: se saltaria eventos o los reprocesaria. */
  intercambiar(): Promise<void>;
}

/** Los flujos del agregado, para recorrerlos. */
export interface FuenteDeFlujos {
  flujos(): Promise<string[]>;
}

/** La vista `conversion`, en `vista_conversion`. Un metodo por evento declarado:
 *  agregar uno al manifiesto rompe la compilacion en vez de dejar la
 *  proyeccion vieja corriendo sin enterarse. */
export interface ConversionProyeccion {
  /** compra.iniciada@v1 · guarda `posicion` en la MISMA transaccion que el efecto */
  aplicarCompraIniciadaV1(e: Envelope<CompraIniciadaV1>, posicion: number): Promise<void>;
  /** compra.cobrada@v1 · guarda `posicion` en la MISMA transaccion que el efecto */
  aplicarCompraCobradaV1(e: Envelope<CompraCobradaV1>, posicion: number): Promise<void>;
  /** compra.compensada@v1 · guarda `posicion` en la MISMA transaccion que el efecto */
  aplicarCompraCompensadaV1(e: Envelope<CompraCompensadaV1>, posicion: number): Promise<void>;
}

export const conversionTabla = "vista_conversion" as const;
export const conversionEventos = ["compra.iniciada@v1", "compra.cobrada@v1", "compra.compensada@v1"] as const;
/** Presupuesto de atraso declarado. Mas viejo que esto no se sirve. */
export const conversionAtrasoMaximoMs = 3000;

/** Rutea el evento al metodo de la vista. El `default` no es
 *  defensivo: un evento que la vista no declara llegaria de una
 *  suscripcion que nadie pidio. */
export async function conversionAplicar(
  proyeccion: ConversionProyeccion,
  e: Envelope<unknown>,
  posicion: number,
): Promise<void> {
  switch (e.type) {
      case "compra.iniciada@v1":
        return proyeccion.aplicarCompraIniciadaV1(e as Envelope<CompraIniciadaV1>, posicion);
      case "compra.cobrada@v1":
        return proyeccion.aplicarCompraCobradaV1(e as Envelope<CompraCobradaV1>, posicion);
      case "compra.compensada@v1":
        return proyeccion.aplicarCompraCompensadaV1(e as Envelope<CompraCompensadaV1>, posicion);
    default:
      throw new Error(`conversion: `+e.type+` no es un evento declarado de la vista`);
  }
}

/** Cuanto atraso lleva la vista, para poder medirlo contra lo declarado. */
export function conversionAtraso(ultimoEvento: Date, ahora = new Date()): number {
  return ahora.getTime() - ultimoEvento.getTime();
}

/** La ruta que reconstruye la vista. NO lleva cron: reconstruir no es
 *  periodico, es una operacion que alguien decide. */
export const rutaReconstruirConversion = "POST /internal/view/conversion/reconstruir" as const;

/** Tira la vista y la vuelve a construir del flujo. Devuelve cuantos
 *  eventos aplico.
 *
 *  Es lo que convierte un modelo de lectura en algo cuya FORMA se puede
 *  cambiar sin migracion: se cambia la proyeccion, se reconstruye, y no
 *  hay `ALTER TABLE` que preserve datos que se pueden recalcular.
 *
 *  Se construye en una SOMBRA y se cambia de golpe al final, asi que
 *  nadie lee un estado intermedio: mientras corre, la vista viva sigue
 *  respondiendo lo de antes. El intercambio toma un bloqueo breve.
 *
 *  El recorrido es por flujo y en orden de version. Una proyeccion cuyo
 *  resultado dependa del orden ENTRE flujos necesita un orden total que
 *  el flujo no tiene: ahi esto da un resultado distinto al de la
 *  proyeccion en vivo, y la comprobacion del demo lo veria. */
export async function reconstruirConversion(
  sombra: ConversionProyeccion & Sombra,
  flujo: FlujoEventos & FuenteDeFlujos,
): Promise<number> {
  await sombra.preparar();
  let aplicados = 0;
  for (const streamId of await flujo.flujos()) {
    for (const ev of await flujo.leer(streamId)) {
      // Los eventos del flujo no son envelopes: se arma el minimo que
      // la proyeccion necesita. El `time` es el del FLUJO, no el de
      // ahora: rellenarlo reescribiria el historial en silencio.
      if (!(conversionEventos as readonly string[]).includes(ev.type)) continue;
      await conversionAplicar(sombra, {
        id: `${streamId}:${ev.version}`,
        type: ev.type,
        source: "reconstruccion",
        time: ev.en,
        traceparent: "",
        correlationId: streamId,
        causationId: null,
        data: ev.data,
      }, ev.version);
      aplicados++;
    }
  }
  // El cambio va al final: hasta aqui nadie vio nada de esto.
  await sombra.intercambiar();
  return aplicados;
}


/** Rutas HTTP que declara el manifiesto. El arranque debe fallar si
 *  alguna no tiene handler: un 404 en produccion no avisa a nadie. */
export const rutasHttp = ["POST /v1/checkouts"] as const;


/** Lado del teorema CAP declarado en el manifiesto: eventual/reject.
 *  De ahi sale el nivel de aislamiento: pagar dos veces sale mas caro
 *  que reintentar, y servir un dato viejo cuesta menos que no servir. */
export const nivelAislamiento = "READ COMMITTED" as const;
/** Presupuesto de obsolescencia: un dato mas viejo que esto no se sirve. */
export const obsolescenciaMaximaMs = 5000;



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

export interface PaymentsRefundPaymentIn {
  paymentId: string;
}

export interface PaymentsRefundPaymentOut {
  paymentId: string;
  status: string;
}

export interface PaymentsPayoutMerchantIn {
  paymentId: string;
  amount: { amount: number; currency: string };
}

export interface PaymentsPayoutMerchantOut {
  payoutId: string;
}


/** Clientes de las dependencias declaradas en el manifiesto. */
export class Clientes {
  protected readonly transporte: Transporte;
  constructor(transporte: Transporte) {
    this.transporte = transporte;
  }
  /** payments.capturePayment · timeout 8000ms · 0 reintentos · breaker true */
  async paymentsCapturePayment(input: PaymentsCapturePaymentIn, e: Envelope<unknown>): Promise<PaymentsCapturePaymentOut> {
    const hacer = () => conPolitica("payments.capturePayment", { timeoutMs: 8000, reintentos: 0, breaker: true }, async () =>
      (await this.transporte.invocar("payments", "capturePayment", input, cabeceras(e, true))) as PaymentsCapturePaymentOut);
    return hacer();
  }
  /** payments.refundPayment · timeout 8000ms · 3 reintentos · breaker true */
  async paymentsRefundPayment(input: PaymentsRefundPaymentIn, e: Envelope<unknown>): Promise<PaymentsRefundPaymentOut> {
    const hacer = () => conPolitica("payments.refundPayment", { timeoutMs: 8000, reintentos: 3, breaker: true }, async () =>
      (await this.transporte.invocar("payments", "refundPayment", input, cabeceras(e, true))) as PaymentsRefundPaymentOut);
    return hacer();
  }
  /** payments.payoutMerchant · timeout 4000ms · 2 reintentos · breaker true */
  async paymentsPayoutMerchant(input: PaymentsPayoutMerchantIn, e: Envelope<unknown>): Promise<PaymentsPayoutMerchantOut> {
    const hacer = () => conPolitica("payments.payoutMerchant", { timeoutMs: 4000, reintentos: 2, breaker: true }, async () =>
      (await this.transporte.invocar("payments", "payoutMerchant", input, cabeceras(e, true))) as PaymentsPayoutMerchantOut);
    return hacer();
  }
}

