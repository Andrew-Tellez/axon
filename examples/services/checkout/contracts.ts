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


/** A saga's journal: where its progress lives. Without it, a restart
 *  halfway through leaves the steps already taken applied and with no
 *  record of which ones: the saga can neither finish nor compensate.
 *
 *  `attempting` is written BEFORE the call and `done` AFTER. A step left
 *  at `attempting` may or may not have happened, so on resume it gets
 *  COMPENSATED, not retried — which is why every compensation has to
 *  tolerate there being nothing to undo.
 *
 *  It also stores the envelope that started the saga. Resuming without it
 *  is impossible: the actions need the call's data, and the process that
 *  held it in memory is precisely the one that died. */
export interface SagaJournal {
  open(id: string, saga: string, e: Envelope<unknown>): Promise<void>;
  mark(id: string, step: number, status: "attempting" | "done" | "undone", output?: unknown): Promise<void>;
  close(id: string, status: SagaStatus): Promise<void>;
  /** How far it got, and what each step returned. `null` if it is new.
   *
   *  The outputs are needed to COMPENSATE: undoing a step usually needs
   *  the id THAT step returned, and after a restart that value is in no
   *  variable — the process that held it is the one that died. Storing it
   *  in the journal is what keeps compensation possible. */
  read(id: string): Promise<{ step: number; status: string; outputs: Record<number, unknown> } | null>;
  /** Claims up to `limit` open sagas that have not moved since
   *  `olderThan`, and returns each one's envelope.
   *
   *  It CLAIMS, it does not list: two instances of the service sweep at
   *  the same time, and two coordinators on the same saga compensate the
   *  same steps twice. In Postgres the claim and the filter are the same
   *  statement:
   *
   *    UPDATE saga_<name> SET updated_at = now()
   *     WHERE status IN ('attempting','done') AND updated_at < $1
   *     RETURNING id, data
   *    LIMIT $2
   *
   *  Touching `updated_at` IS the claim: the other sweeper no longer sees
   *  it. And if this process dies halfway, the saga becomes eligible again
   *  in the next window with nobody unlocking it by hand. */
  claim(saga: string, olderThan: Date, limit: number): Promise<{ id: string; data: Envelope<unknown> }[]>;
}

export type SagaStatus = "completed" | "compensated" | "stuck";

/** A compensation that fails has nothing behind it: the saga is left
 *  half-done and needs a person. It is thrown so that does not go
 *  unnoticed. */
export class SagaStuck extends Error {
  readonly saga: string;
  readonly step: number;
  readonly reason: unknown;
  constructor(saga: string, step: number, reason: unknown) {
    super(`${saga}: compensating step ${step} failed; the saga is half-done`);
    this.saga = saga;
    this.step = step;
    this.reason = reason;
  }
}

/** What one sweep pass did. It is returned so it can be measured: a
 *  sweep that reports nothing is indistinguishable from one that does not
 *  run. */
export interface SweepReport {
  claimed: number;
  completed: number;
  compensated: number;
  /** These need a person. The sweep does NOT retry them. */
  stuck: number;
  /** Left for the next pass because the limit was reached. */
  pending: boolean;
}

/** The route the scheduler hits to run one sweep pass. `axon infra`
 *  deploys it on all four targets, so startup has to serve it by calling
 *  `sweepCompra`: a scheduler pointed at a 404 applies without an error and
 *  sweeps nothing.
 *
 *  It is NOT a declared method, so it does not go out through the
 *  gateway. It triggers compensations: it cannot be public. */
export const sweepRouteCompra = "POST /internal/saga/compra/sweep" as const;

/** The steps declared in the manifest. Generated: do not edit. */
export const compraSteps = [
  { step: 1, run: "payments.capturePayment", undo: "payments.refundPayment" },
  // the last step carries no compensation: if it fails, there is
  // nothing of its own to undo
  { step: 2, run: "payments.payoutMerchant", undo: null },
] as const;

/** What each step returned, stored in the journal. It is what makes
 *  compensating possible after a restart: undoing a step usually needs
 *  the id that step returned, and a variable in memory does not survive
 *  the process that held it. */
export interface CompraOutputs {
  step1?: PaymentsCapturePaymentOut;
  step2?: PaymentsPayoutMerchantOut;
}

/** One method per step and one per compensation. They are implemented
 *  by whoever knows the data: the coordinator knows the order, not the
 *  contents. */
export interface CompraActions {
  /** step 1 · payments.capturePayment */
  step1CapturePayment(e: Envelope<unknown>, prior: CompraOutputs): Promise<PaymentsCapturePaymentOut>;
  /** undoes step 1 · payments.refundPayment · receives what the earlier steps returned,
   *  and has to tolerate there being nothing to undo */
  undo1RefundPayment(e: Envelope<unknown>, prior: CompraOutputs): Promise<void>;
  /** step 2 · payments.payoutMerchant */
  step2PayoutMerchant(e: Envelope<unknown>, prior: CompraOutputs): Promise<PaymentsPayoutMerchantOut>;
}

/** Runs the `compra` saga.
 *
 *  Forward until a step fails or the budget runs out; from there in
 *  REVERSE order, undoing only what was attempted. Reverse order is not
 *  aesthetics: compensating forward undoes a step whose effect a later
 *  step already used.
 *
 *  If `id` already has a journal, it resumes: the step left at
 *  `attempting` gets compensated, because there is no telling whether it
 *  happened. */
export async function runCompra(
  id: string,
  actions: CompraActions,
  journal: SagaJournal,
  e: Envelope<unknown>,
): Promise<{ status: SagaStatus; upTo: number; error?: unknown }> {
  const total = 2;
  // The budget declared in the manifest; `axon verify` already checked
  // that it covers the sum of the steps and their compensations.
  const deadline = Date.now() + 60000;
  const previous = await journal.read(id);
  if (!previous) await journal.open(id, "compra", e);
  // Rehydrated from the journal, not from a variable: on resume this is
  // all that is left of what the earlier steps did.
  //
  // The journal stores them by step NUMBER and here they are named: a
  // cast from one shape to the other compiles and leaves everything
  // `undefined`, so the translation is explicit, field by field.
  const saved = previous?.outputs ?? {};
  const prior: CompraOutputs = {
    step1: saved[1] as PaymentsCapturePaymentOut | undefined,
    step2: saved[2] as PaymentsPayoutMerchantOut | undefined,
  };
  // a half-attempted step is not retried: it is undone
  let done = previous ? (previous.status === "done" ? previous.step : previous.step - 1) : 0;
  const doubtful = previous?.status === "attempting" ? previous.step : 0;
  // The step that FAILED gets undone too: a timeout does not say that
  // nothing happened on the other side. Compensating only up to the last
  // success leaves that effect applied forever.
  let attempted = doubtful;
  let failure: unknown = doubtful ? new Error("resumed with a step in doubt") : undefined;
  if (!doubtful) {
    for (let step = done + 1; step <= total; step++) {
      if (Date.now() > deadline) {
        failure = new Error(`compra: budget exhausted before step ${step}`);
        break;
      }
      attempted = step;
      try {
        await runStepCompra(step, actions, journal, e, id, prior);
        done = step;
      } catch (err) {
        failure = err;
        break;
      }
    }
  }
  if (!failure) {
    await journal.close(id, "completed");
    return { status: "completed", upTo: total };
  }
  // back again: everything that was ATTEMPTED, in reverse order
  for (let step = attempted; step >= 1; step--) {
    try {
      await undoStepCompra(step, actions, e, prior);
      await journal.mark(id, step, "undone");
    } catch (err) {
      await journal.close(id, "stuck");
      throw new SagaStuck("compra", step, err);
    }
  }
  await journal.close(id, "compensated");
  return { status: "compensated", upTo: done, error: failure };
}

async function runStepCompra(
  step: number,
  actions: CompraActions,
  journal: SagaJournal,
  e: Envelope<unknown>,
  id: string,
  prior: CompraOutputs,
): Promise<void> {
  switch (step) {
      case 1:
        await journal.mark(id, 1, "attempting");
        prior.step1 = await actions.step1CapturePayment(e, prior);
        // the output is stored WITH the `done`: in two writes, a crash
        // between them leaves the step done and its result lost
        await journal.mark(id, 1, "done", prior.step1);
        break;
      case 2:
        await journal.mark(id, 2, "attempting");
        prior.step2 = await actions.step2PayoutMerchant(e, prior);
        // the output is stored WITH the `done`: in two writes, a crash
        // between them leaves the step done and its result lost
        await journal.mark(id, 2, "done", prior.step2);
        break;
    default:
      throw new Error(`compra: step ${step} is not declared in the manifest`);
  }
}

async function undoStepCompra(step: number, actions: CompraActions, e: Envelope<unknown>, prior: CompraOutputs): Promise<void> {
  switch (step) {
      case 1:
        await actions.undo1RefundPayment(e, prior);
        break;
      case 2:
        break; // no compensation declared: this is the last step
    default:
      throw new Error(`compra: step ${step} is not declared in the manifest`);
  }
}

/** One sweep pass: resumes the `compra` sagas that are not moving.
 *
 *  It only touches those idle for longer than their own BUDGET
 *  (60000ms). That threshold is not a heuristic: `axon verify` already
 *  checked that the budget covers the sum of the steps and their
 *  compensations, so a saga older than that is not on its way — it is
 *  stranded. Sweeping sooner would mean running a second coordinator over
 *  a live saga.
 *
 *  One left `stuck` is NOT retried: it is counted and left alone. A
 *  compensation that already failed needs a person, and retrying it
 *  quietly hides exactly that. */
export async function sweepCompra(
  actions: CompraActions,
  journal: SagaJournal,
  limit = 50,
): Promise<SweepReport> {
  const olderThan = new Date(Date.now() - 60000);
  const stranded = await journal.claim("compra", olderThan, limit);
  const r: SweepReport = {
    claimed: stranded.length,
    completed: 0,
    compensated: 0,
    stuck: 0,
    // If the limit filled up there are more waiting, and saying so is
    // the difference between a sweep that keeps up and one that does not.
    pending: stranded.length >= limit,
  };
  for (const { id, data } of stranded) {
    try {
      const out = await runCompra(id, actions, journal, data);
      if (out.status === "completed") r.completed++;
      else r.compensated++;
    } catch (err) {
      // A stuck saga does not abort the pass: the others are still
      // stranded and this is the only thing that will look at them.
      if (err instanceof SagaStuck) r.stuck++;
      else throw err;
    }
  }
  return r;
}

/** Starts the periodic sweep and returns how to stop it.
 *
 *  The interval comes from the declared budget: nothing becomes eligible
 *  sooner, so sweeping more often is work without a result. It is safe
 *  with several instances because `claim` claims.
 *
 *  `onPass` receives every pass. Wire it to your metrics: a sweep that
 *  reports nothing is indistinguishable from one that does not run, and
 *  this is the only place from which a stuck saga becomes visible. */
export function startSweepCompra(
  actions: CompraActions,
  journal: SagaJournal,
  onPass: (r: SweepReport) => void,
  intervalMs = 60000,
): () => void {
  let running = false;
  const t = setInterval(async () => {
    // Without this, a slow pass overlaps the next one in the same
    // process and both of them claim.
    if (running) return;
    running = true;
    try {
      onPass(await sweepCompra(actions, journal));
    } finally {
      running = false;
    }
  }, intervalMs);
  // a background sweep should not keep the process alive on shutdown
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


/** HTTP routes the manifest declares. Startup must fail if any of them
 *  has no handler: a 404 in production tells nobody. */
export const httpRoutes = ["POST /v1/checkouts"] as const;


/** The CAP side declared in the manifest: eventual/reject.
 *  The isolation level follows from it: paying twice costs more than
 *  retrying, and serving stale data costs less than serving nothing. */
export const isolationLevel = "READ COMMITTED" as const;
/** Staleness budget: data older than this does not get served. */
export const maxStalenessMs = 5000;



/** Everything needed to reach another service. Implemented by whoever
 *  deploys: HTTP, gRPC, an SDK. The framework does not pick a transport. */
export interface Transport {
  call(target: string, method: string, body: unknown, headers: Record<string, string>): Promise<unknown>;
}

export class TimedOut extends Error {}
export class CircuitOpen extends Error {}

/** The policy declared in the manifest. The generator emits it; nobody types it. */
export interface Policy {
  timeoutMs: number;
  retries: number;
  breaker: boolean;
}

/** One breaker per target: when the other side goes down, stop hitting it.
 *  After the cooldown it goes half-open and tries exactly once. */
class Breaker {
  #failures = 0;
  #openUntil = 0;
  // explicit fields: parameter properties are TypeScript-only and do not
  // survive Node's type stripping
  readonly threshold: number;
  readonly cooldownMs: number;
  constructor(threshold = 5, cooldownMs = 10_000) {
    this.threshold = threshold;
    this.cooldownMs = cooldownMs;
  }

  allows(now: number) {
    return this.#openUntil === 0 || now >= this.#openUntil;
  }
  succeeded() {
    this.#failures = 0;
    this.#openUntil = 0;
  }
  failed(now: number) {
    this.#failures++;
    if (this.#failures >= this.threshold) this.#openUntil = now + this.cooldownMs;
  }
}

const breakers = new Map<string, Breaker>();

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function withTimeout<T>(p: Promise<T>, ms: number, who: string): Promise<T> {
  let t: ReturnType<typeof setTimeout>;
  const limit = new Promise<never>((_, reject) => {
    t = setTimeout(() => reject(new TimedOut(`${who}: timed out after ${ms}ms`)), ms);
  });
  try {
    return await Promise.race([p, limit]);
  } finally {
    clearTimeout(t!);
  }
}

/** Applies the declared policy. Retries are only emitted for idempotent
 *  methods: `axon verify` blocks the rest. */
export async function withPolicy<T>(who: string, pol: Policy, attempt: () => Promise<T>): Promise<T> {
  const breaker = pol.breaker
    ? (breakers.get(who) ?? breakers.set(who, new Breaker()).get(who)!)
    : null;
  if (breaker && !breaker.allows(Date.now())) {
    throw new CircuitOpen(`${who}: circuit open`);
  }
  let last: unknown;
  for (let n = 0; n <= pol.retries; n++) {
    try {
      const r = await withTimeout(attempt(), pol.timeoutMs, who);
      breaker?.succeeded();
      return r;
    } catch (err) {
      last = err;
      breaker?.failed(Date.now());
      if (n === pol.retries) break;
      // exponential with full jitter: without jitter every client retries at
      // the same instant and the other side never comes back up
      const ceiling = Math.min(1000 * 2 ** n, 10_000);
      await sleep(Math.random() * ceiling);
    }
  }
  throw last;
}

/** Headers of an outgoing call: the trace stays the same one. */
export function headers(e: Envelope<unknown>, idempotent: boolean): Record<string, string> {
  const h: Record<string, string> = {
    traceparent: e.traceparent,
    "x-correlation-id": e.correlationId,
    "x-causation-id": e.id,
  };
  // retrying without a key would duplicate the effect on the other side
  if (idempotent) h["idempotency-key"] = e.id;
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


/** Clients for the dependencies declared in the manifest. */
export class Clients {
  protected readonly transport: Transport;
  constructor(transport: Transport) {
    this.transport = transport;
  }
  /** payments.capturePayment · timeout 8000ms · 0 retries · breaker true */
  async paymentsCapturePayment(input: PaymentsCapturePaymentIn, e: Envelope<unknown>): Promise<PaymentsCapturePaymentOut> {
    const attempt = () => withPolicy("payments.capturePayment", { timeoutMs: 8000, retries: 0, breaker: true }, async () =>
      (await this.transport.call("payments", "capturePayment", input, headers(e, true))) as PaymentsCapturePaymentOut);
    return attempt();
  }
  /** payments.refundPayment · timeout 8000ms · 3 retries · breaker true */
  async paymentsRefundPayment(input: PaymentsRefundPaymentIn, e: Envelope<unknown>): Promise<PaymentsRefundPaymentOut> {
    const attempt = () => withPolicy("payments.refundPayment", { timeoutMs: 8000, retries: 3, breaker: true }, async () =>
      (await this.transport.call("payments", "refundPayment", input, headers(e, true))) as PaymentsRefundPaymentOut);
    return attempt();
  }
  /** payments.payoutMerchant · timeout 4000ms · 2 retries · breaker true */
  async paymentsPayoutMerchant(input: PaymentsPayoutMerchantIn, e: Envelope<unknown>): Promise<PaymentsPayoutMerchantOut> {
    const attempt = () => withPolicy("payments.payoutMerchant", { timeoutMs: 4000, retries: 2, breaker: true }, async () =>
      (await this.transport.call("payments", "payoutMerchant", input, headers(e, true))) as PaymentsPayoutMerchantOut);
    return attempt();
  }
}

