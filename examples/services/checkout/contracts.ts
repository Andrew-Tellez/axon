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
 *  cambio de estado, y un relay lo publica despues. Sin dual-write. */
export interface Outbox { stage(e: Envelope<unknown>): Promise<void>; }

/** Inbox / consumidor idempotente: el broker entrega al menos una vez, el
 *  efecto ocurre una sola. `once` no reejecuta un id ya visto. */
export interface Inbox { once(id: string, fn: () => Promise<void>): Promise<void>; }

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
  "emits": {},
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
    "outbox": false
  },
  "cap": {
    "consistency": "eventual",
    "on_partition": "reject",
    "max_staleness_ms": 5000
  },
  "flags": {},
  "analytics": {
    "export": true,
    "pii": "exclude"
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

  static readonly wellKnown = "/.well-known/axon.json";
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

