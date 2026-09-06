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


/** An event as it sits in the stream. `at` is WHEN IT HAPPENED, and it
 *  travels because rebuilding a view has to put it back: filling it in with
 *  the rebuild's clock rewrites history in silence. */
export interface StreamEvent {
  version: number;
  type: string;
  data: unknown;
  at: string;
}

/** An aggregate's stream. It is the source of truth: what today sits in a
 *  row is a PROJECTION of this.
 *
 *  `append` takes the version the caller believed was current. If someone
 *  else wrote in between, it has to fail: that is the UNIQUE (stream_id,
 *  version) doing its job, and `axon verify` checks that it exists — because
 *  without it both writes land and nobody sees an error. */
export interface EventStream {
  /** A stream's events, in order. `from` allows starting at a snapshot. */
  read(streamId: string, from?: number): Promise<StreamEvent[]>;
  /** Appends at the end. Rejects if `expected` is no longer the last version. */
  append(streamId: string, expected: number, e: Envelope<unknown>): Promise<number>;
  /** Snapshots live in `SnapshottingStream`, generated only when the
   *  manifest declares them: optional here means declaring snapshots and
   *  not implementing them would compile and do nothing. */
}

/** Someone else wrote first. Not a programming error: it is the normal
 *  condition of two users on the same aggregate, and whoever gets it reads
 *  again and retries. */
export class VersionConflict extends Error {
  readonly streamId: string;
  readonly expected: number;
  constructor(streamId: string, expected: number) {
    super(`${streamId}: version ${expected} is no longer the last one`);
    this.streamId = streamId;
    this.expected = expected;
  }
}

/** A stream that also stores snapshots.
 *
 *  A snapshot is a CACHE of the `fold`, which is why it carries the
 *  version of the rules it was computed with: if the `fold` changes, the
 *  old snapshots encode the previous version, and rehydrating from one
 *  gives a state that no longer matches replaying the stream. That does
 *  not raise an error: it returns a wrong number.
 *
 *  `snapshot` only returns those of the current version. Ones from another
 *  version are ignored and the state is rebuilt from scratch, which is
 *  slow and correct — in that order. */
export interface SnapshottingStream extends EventStream {
  snapshot(streamId: string, rules: number): Promise<{ version: number; state: unknown } | null>;
  saveSnapshot(streamId: string, version: number, rules: number, state: unknown): Promise<void>;
  /** Deletes the snapshots the current version does not use: those of
   *  ANOTHER rules version, and all but the newest of each stream.
   *  Returns how many it deleted.
   *
   *  It can afford to be aggressive precisely because a snapshot is a
   *  CACHE: the worst that can happen is rebuilding from the stream,
   *  which is slow and correct. Deleting too much breaks nothing; never
   *  deleting grows the table with every rules version.
   *
   *  And there is no race with whoever is rehydrating: `snapshot` returns
   *  the state BY VALUE, so deleting the row afterwards takes nothing
   *  away from them. */
  pruneSnapshots(rules: number): Promise<number>;
}

/** The events that make up `compra`, as declared in the manifest. */
export const compraEvents = ["compra.iniciada@v1", "compra.cobrada@v1", "compra.compensada@v1"] as const;
export type CompraEvent = typeof compraEvents[number];

/** The rebuilt state. Its shape is the domain's; what the generator
 *  enforces is that there is one case per declared event. */
export interface CompraRules<CompraState> {
  /** The state before the first event. */
  initial(streamId: string): CompraState;
  applyCompraIniciadaV1(state: CompraState, e: CompraIniciadaV1): CompraState;
  applyCompraCobradaV1(state: CompraState, e: CompraCobradaV1): CompraState;
  applyCompraCompensadaV1(state: CompraState, e: CompraCompensadaV1): CompraState;
}

/** Rebuilds the state by applying the stream in order. */
export function compraFold<E>(
  rules: CompraRules<E>,
  streamId: string,
  events: StreamEvent[],
  from?: { version: number; state: E },
): { version: number; state: E } {
  let state = from ? from.state : rules.initial(streamId);
  let version = from ? from.version : 0;
  for (const ev of events) {
    // The order is not assumed: a gap in the versions means an event is
    // missing, and rebuilding without it gives a state that never
    // existed.
    if (ev.version !== version + 1) {
      throw new Error(`compra/${streamId}: expected version ${version + 1} and got ${ev.version}`);
    }
    state = compraApplyEvent(rules, state, ev);
    version = ev.version;
  }
  return { version, state };
}

function compraApplyEvent<E>(rules: CompraRules<E>, state: E, ev: { type: string; data: unknown }): E {
  switch (ev.type) {
      case "compra.iniciada@v1":
        return rules.applyCompraIniciadaV1(state, ev.data as CompraIniciadaV1);
      case "compra.cobrada@v1":
        return rules.applyCompraCobradaV1(state, ev.data as CompraCobradaV1);
      case "compra.compensada@v1":
        return rules.applyCompraCompensadaV1(state, ev.data as CompraCompensadaV1);
    default:
      // An event in the stream the manifest does not declare: the state
      // that would come out of ignoring it is wrong and nobody would know.
      throw new Error(`compra: `+ev.type+` is not a declared event of the aggregate`);
  }
}

/** How many events between snapshots, and with which rules version.
 *  Both numbers come from the manifest: nobody types them twice. */
export const compraSnapshotEvery = 2;
export const compraSnapshotRules = 1;

/** Loads the state: from the last valid snapshot, and only the rest
 *  of the stream from there.
 *
 *  With no snapshot of the current version it rebuilds the whole
 *  thing. That is slow and correct, in that order: a snapshot from
 *  another version would give a wrong state without saying so. */
export async function compraLoad<E>(
  rules: CompraRules<E>,
  stream: SnapshottingStream,
  streamId: string,
): Promise<{ version: number; state: E }> {
  const s = await stream.snapshot(streamId, compraSnapshotRules);
  const from = s ? { version: s.version, state: s.state as E } : undefined;
  const events = await stream.read(streamId, s?.version ?? 0);
  return compraFold(rules, streamId, events, from);
}

/** Snapshots if it is time to. Returns whether it saved one, so it
 *  can be measured: a declared cadence that is not met is a snapshot
 *  nobody knows is missing. */
export async function compraSnapshot<E>(
  stream: SnapshottingStream,
  streamId: string,
  version: number,
  state: E,
): Promise<boolean> {
  if (version === 0 || version % compraSnapshotEvery !== 0) return false;
  await stream.saveSnapshot(streamId, version, compraSnapshotRules, state);
  return true;
}

/** The route the scheduler hits to prune the old snapshots.
 *  `axon infra` deploys it on all four targets. */
export const pruneRouteCompra = "POST /internal/aggregate/compra/prune" as const;

/** One prune pass. Returns how many snapshots it deleted, so it can
 *  be measured: a prune that reports nothing is indistinguishable from
 *  one that does not run, and what shows up then is the table size. */
export async function pruneCompra(stream: SnapshottingStream): Promise<number> {
  return stream.pruneSnapshots(compraSnapshotRules);
}


/** Where a view records how far it got.
 *
 *  Without this, a restart either reprocesses from the beginning or skips
 *  what it did not get to apply. Both give a wrong view, and neither raises
 *  an error: that is why `axon verify` demands the table. */
export interface Checkpoint {
  /** Where to resume from. The WRITE is deliberately not here: the
   *  position has to be stored in the same transaction as the view's
   *  effect, and that transaction belongs to the projection, not to the
   *  framework. In two transactions, a crash between them leaves the view
   *  ahead of or behind what it claims to have applied, and neither of
   *  those raises an error.
   *
   *  That is why every `apply*` receives the position: it is stored by
   *  whoever can do it alongside the rest.
   *
   *  And it is PER STREAM. An event's version is its position inside its
   *  own stream, so a single number for the whole view identifies nothing
   *  as soon as there is more than one stream: with one it seemed to
   *  work. */
  read(view: string, streamId: string): Promise<number>;
}

/** A shadow view: built aside and swapped for the live one all at
 *  once.
 *
 *  Rebuilding in place leaves the view incomplete while it runs, and it
 *  keeps being read: whoever asks gets fewer rows than there are, with no
 *  error. With a shadow, nobody sees an intermediate state.
 *
 *  The projection handed to `rebuild` has to be pointed at the SHADOW. If
 *  it pointed at the live one this would be an in-place rebuild with extra
 *  steps — which is why the demo measures that reads never drop while it
 *  runs. */
export interface Shadow {
  /** Leaves the shadow empty, with its position at zero. */
  prepare(): Promise<void>;
  /** Swaps the shadow for the live one, and its position with it, in ONE
   *  transaction. In two, a crash between them leaves the new table with
   *  the old position: it would skip events or reprocess them. */
  swap(): Promise<void>;
}

/** The aggregate's streams, so they can be walked. */
export interface StreamSource {
  streams(): Promise<string[]>;
}

/** The `conversion` view, in `vista_conversion`. One method per declared event:
 *  adding one to the manifest breaks compilation instead of leaving the
 *  old projection running none the wiser. */
export interface ConversionProjection {
  /** compra.iniciada@v1 · stores `position` in the SAME transaction as the effect */
  applyCompraIniciadaV1(e: Envelope<CompraIniciadaV1>, position: number): Promise<void>;
  /** compra.cobrada@v1 · stores `position` in the SAME transaction as the effect */
  applyCompraCobradaV1(e: Envelope<CompraCobradaV1>, position: number): Promise<void>;
  /** compra.compensada@v1 · stores `position` in the SAME transaction as the effect */
  applyCompraCompensadaV1(e: Envelope<CompraCompensadaV1>, position: number): Promise<void>;
}

export const conversionTable = "vista_conversion" as const;
export const conversionEvents = ["compra.iniciada@v1", "compra.cobrada@v1", "compra.compensada@v1"] as const;
/** The declared staleness budget. Older than this does not get served. */
export const conversionMaxStalenessMs = 3000;

/** Routes the event to the view's method. The `default` is not
 *  defensive: an event the view does not declare would come from a
 *  subscription nobody asked for. */
export async function conversionApply(
  projection: ConversionProjection,
  e: Envelope<unknown>,
  position: number,
): Promise<void> {
  switch (e.type) {
      case "compra.iniciada@v1":
        return projection.applyCompraIniciadaV1(e as Envelope<CompraIniciadaV1>, position);
      case "compra.cobrada@v1":
        return projection.applyCompraCobradaV1(e as Envelope<CompraCobradaV1>, position);
      case "compra.compensada@v1":
        return projection.applyCompraCompensadaV1(e as Envelope<CompraCompensadaV1>, position);
    default:
      throw new Error(`conversion: `+e.type+` is not a declared event of the view`);
  }
}

/** How far behind the view is, so it can be measured against what was
 *  declared. */
export function conversionLag(lastEvent: Date, now = new Date()): number {
  return now.getTime() - lastEvent.getTime();
}

/** The route that rebuilds the view. It carries NO cron: rebuilding
 *  is not periodic, it is an operation somebody decides on. */
export const rebuildRouteConversion = "POST /internal/view/conversion/rebuild" as const;

/** Throws the view away and builds it again from the stream. Returns
 *  how many events it applied.
 *
 *  This is what turns a read model into something whose SHAPE can be
 *  changed without a migration: change the projection, rebuild, and
 *  there is no `ALTER TABLE` preserving data that can be recomputed.
 *
 *  It is built in a SHADOW and swapped at the very end, so nobody reads
 *  an intermediate state: while it runs, the live view keeps answering
 *  what it did before. The swap takes a brief lock.
 *
 *  The walk is per stream and in version order. A projection whose
 *  result depends on the order BETWEEN streams needs a total order the
 *  stream does not have: there this would give a different result from
 *  the live projection, and the demo's check would see it. */
export async function rebuildConversion(
  shadow: ConversionProjection & Shadow,
  stream: EventStream & StreamSource,
): Promise<number> {
  await shadow.prepare();
  let applied = 0;
  for (const streamId of await stream.streams()) {
    for (const ev of await stream.read(streamId)) {
      // Stream events are not envelopes: the minimum the projection
      // needs gets built here. The `time` is the STREAM's, not now's:
      // filling it in would rewrite history in silence.
      if (!(conversionEvents as readonly string[]).includes(ev.type)) continue;
      await conversionApply(shadow, {
        id: `${streamId}:${ev.version}`,
        type: ev.type,
        source: "rebuild",
        time: ev.at,
        traceparent: "",
        correlationId: streamId,
        causationId: null,
        data: ev.data,
      }, ev.version);
      applied++;
    }
  }
  // The swap goes at the end: until here nobody saw any of this.
  await shadow.swap();
  return applied;
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

