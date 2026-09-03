#!/usr/bin/env python3
"""axon - manifest-first compiler for event-driven microservices.

El manifiesto es la fuente de verdad. Todo lo demas (codigo, IaC, docs,
topologia) es una proyeccion, y `axon verify` falla cuando dejan de coincidir.
Sin dependencias: TOML via tomllib (stdlib, py3.11+).
"""
import argparse, json, re, sys, tomllib, urllib.request
from pathlib import Path

# ---------- modelo ----------

SCALARS = {
    "string": "string", "int": "number", "float": "number", "bool": "boolean",
    "timestamp": "string", "uuid": "string", "json": "unknown",
    "money": "{ amount: number; currency: string }",
}

def load(path):
    m = tomllib.loads(Path(path).read_text())
    m["_path"] = str(path)
    if "service" not in m:
        die(f"{path}: falta `service`")
    return m

def load_dir(d):
    return [load(p) for p in sorted(Path(d).glob("*.toml"))]

def die(msg):
    print(f"axon: {msg}", file=sys.stderr)
    sys.exit(1)

def pascal(event):
    """payment.captured@v1 -> PaymentCapturedV1"""
    return "".join(w[0].upper() + w[1:] for w in re.split(r"[.@_-]", event) if w)

def camel(s):
    parts = re.split(r"[.@_-]", s)
    return parts[0] + "".join(w[0].upper() + w[1:] for w in parts[1:] if w)

def ts(t):
    return SCALARS.get(t, "unknown")

# ---------- build: codigo ----------

ENVELOPE_TS = '''
/** Envelope CloudEvents + cadena causal. La trazabilidad no es opcional:
 *  ningun mensaje sale del proceso sin traceparent, correlationId y causationId. */
export interface Envelope<T> {
  id: string;
  type: string;
  source: string;
  time: string;
  traceparent: string;      // W3C: 00-<trace>-<span>-<flags>
  correlationId: string;    // estable en todo el flujo de negocio
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
 *  efecto ocurre una sola. `once` no vuelve a ejecutar un id ya visto. */
export interface Inbox { once(id: string, fn: () => Promise<void>): Promise<void>; }
'''

def iface(name, fields):
    body = "".join(f"  {k}: {ts(v)};\n" for k, v in fields.items())
    return f"export interface {name} {{\n{body}}}\n"

def build_ts(m):
    svc = m["service"]
    out = [f"// generado por axon desde {Path(m['_path']).name} — no editar\n", ENVELOPE_TS]

    for ev, fields in m.get("emits", {}).items():
        out.append(iface(pascal(ev), fields))
    for meth, spec in m.get("methods", {}).items():
        out.append(iface(pascal(meth) + "In", spec.get("in", {})))
        out.append(iface(pascal(meth) + "Out", spec.get("out", {})))

    # manifiesto servible: asi otros servicios lo descubren en caliente
    out.append(f"export const manifest = {json.dumps(public(m), indent=2)} as const;\n")

    pats = m.get("patterns", {})
    outbox = pats.get("outbox", False)
    ctor = "  constructor(protected readonly bus: Bus, protected readonly inbox: Inbox"
    ctor += ", protected readonly outbox: Outbox) {}" if outbox else ") {}"
    cls = [f"export abstract class {pascal(svc)}Service {{", ctor,
           f'  static readonly wellKnown = "/.well-known/axon.json";']
    for ev in m.get("emits", {}):
        cls.append(f"  protected {camel('emit.' + ev)}(data: {pascal(ev)}, cause?: Envelope<unknown>) {{")
        sink = "this.outbox.stage" if outbox else "this.bus.publish"
        cls.append(f'    return {sink}(newEnvelope("{ev}", "{svc}", data, cause));')
        cls.append("  }")
    for ev, spec in m.get("consumes", {}).items():
        cls.append(f"  /** consume {ev} */")
        cls.append(f"  abstract {spec['handler']}(e: Envelope<{pascal(ev)}>): Promise<void>;")
    for meth, spec in m.get("methods", {}).items():
        cls.append(f"  abstract {camel(meth)}(input: {pascal(meth)}In, e: Envelope<unknown>): Promise<{pascal(meth)}Out>;")
    if m.get("consumes"):
        cls.append("  /** Punto de entrada unico: rutea por tipo y deduplica por id de envelope. */")
        cls.append("  dispatch(e: Envelope<unknown>): Promise<void> {")
        cls.append("    return this.inbox.once(e.id, async () => {")
        cls.append("      switch (e.type) {")
        for ev, spec in m["consumes"].items():
            cls.append(f'        case "{ev}": return this.{spec["handler"]}(e as Envelope<{pascal(ev)}>);')
        cls.append("        default: throw new Error(`" + svc + ": tipo no declarado en el manifiesto: ${e.type}`);")
        cls.append("      }\n    });\n  }")
    cls.append("}\n")
    out.append("\n".join(cls))
    return "\n".join(out)

# ponytail: solo target TS. Otro lenguaje = otra funcion build_X, no un motor de plantillas.
BUILDERS = {"ts": build_ts}

# ---------- infra: IaC ----------

def build_infra(manifests):
    """Pub/Sub + DLQ + estado + secretos. Todo derivado del manifiesto."""
    tf = ['# generado por axon — no editar\n']
    emitted = {ev for m in manifests for ev in m.get("emits", {})}
    for ev in sorted(emitted):
        n = tfname(ev)
        tf.append(f'resource "google_pubsub_topic" "{n}" {{\n  name = "{topic(ev)}"\n}}\n')
        tf.append(f'resource "google_pubsub_topic" "{n}_dlq" {{\n  name = "{topic(ev)}.dlq"\n}}\n')
    for m in manifests:
        svc = m["service"]
        for ev in m.get("consumes", {}):
            n = f"{tfname(svc)}_{tfname(ev)}"
            tf.append(
                f'resource "google_pubsub_subscription" "{n}" {{\n'
                f'  name  = "{svc}--{topic(ev)}"\n'
                f'  topic = google_pubsub_topic.{tfname(ev)}.name\n'
                f'  dead_letter_policy {{\n'
                f'    dead_letter_topic     = google_pubsub_topic.{tfname(ev)}_dlq.id\n'
                f'    max_delivery_attempts = 5\n  }}\n}}\n')
        infra = m.get("infra", {})
        if infra.get("state") == "postgres":
            tf.append(f'resource "google_sql_database" "{tfname(svc)}" {{\n'
                      f'  name     = "{svc}"\n  instance = var.sql_instance\n}}\n')
        if m.get("patterns", {}).get("outbox"):
            tf.append(f'resource "google_sql_user" "{tfname(svc)}_relay" {{\n'
                      f'  name     = "{svc}-outbox-relay"\n  instance = var.sql_instance\n}}\n')
            tf.append(f'# tabla outbox de {svc}: migracion generada, publicada por el relay\n'
                      f'# CREATE TABLE {tfname(svc)}_outbox (id uuid primary key, type text, payload jsonb,\n'
                      f'#   traceparent text, correlation_id uuid, causation_id uuid, published_at timestamptz);\n')
        for s in infra.get("secrets", []):
            tf.append(f'resource "google_secret_manager_secret" "{tfname(svc)}_{tfname(s)}" {{\n'
                      f'  secret_id = "{svc}-{s.lower().replace("_","-")}"\n'
                      f'  replication {{ auto {{}} }}\n}}\n')
    return "\n".join(tf)

def topic(ev):
    """Pub/Sub no admite '@' en nombres."""
    return ev.replace("@", ".")

def tfname(s):
    return re.sub(r"[^a-z0-9]+", "_", s.lower()).strip("_")

# ---------- graph ----------

def build_graph(manifests):
    lines = ["graph LR"]
    for m in manifests:
        shape = "([{}])" if m.get("external") else "[{}]"
        lines.append(f"  {tfname(m['service'])}{shape.format(m['service'])}")
    for m in manifests:
        src = tfname(m["service"])
        for ev in m.get("emits", {}):
            lines.append(f"  {src} -- {ev} --> {tfname(ev)}(({ev}))")
        for ev in m.get("consumes", {}):
            lines.append(f"  {tfname(ev)}(({ev})) --> {src}")
        for d in m.get("depends", []):
            tgt = tfname(d.get("service") or d["external"])
            lines.append(f"  {src} -. {d['method']} .-> {tgt}")
    return "\n".join(lines)

# ---------- discover ----------

def public(m):
    return {k: v for k, v in m.items() if not k.startswith("_")}

def discover(sources, timeout=5):
    """Fusiona manifiestos locales y remotos. Un servicio vivo publica el suyo
    en /.well-known/axon.json; un externo se congela en un *.external.toml."""
    found = []
    for s in sources:
        if s.startswith(("http://", "https://")):
            url = s if s.endswith(".json") else s.rstrip("/") + "/.well-known/axon.json"
            try:
                with urllib.request.urlopen(url, timeout=timeout) as r:
                    d = json.load(r)
                d["_path"] = url
                found.append(d)
            except Exception as e:  # un servicio caido no rompe el descubrimiento
                print(f"axon: {url}: {e}", file=sys.stderr)
        elif Path(s).is_dir():
            found.extend(load_dir(s))
        else:
            found.append(load(s))
    return found

def registry(manifests):
    return {
        m["service"]: {
            "version": m.get("version"),
            "external": bool(m.get("external")),
            "source": m["_path"],
            "methods": {k: v for k, v in m.get("methods", {}).items()},
            "emits": sorted(m.get("emits", {})),
            "consumes": sorted(m.get("consumes", {})),
        }
        for m in manifests
    }

# ---------- verify: drift ----------

def verify(manifests):
    """El chequeo que justifica el framework: manifiesto vs manifiesto."""
    errors, warnings = [], []
    emitters = {}
    for m in manifests:
        for ev, fields in m.get("emits", {}).items():
            if ev in emitters and emitters[ev][1] != fields:
                errors.append(f"{ev}: dos emisores con esquemas distintos "
                              f"({emitters[ev][0]} vs {m['service']})")
            emitters[ev] = (m["service"], fields)

    known = {m["service"]: m for m in manifests}
    for m in manifests:
        svc = m["service"]
        for ev in m.get("consumes", {}):
            if ev not in emitters:
                errors.append(f"{svc} consume {ev} pero nadie lo emite")
        for d in m.get("depends", []):
            tgt = d.get("service") or d.get("external")
            if tgt not in known:
                errors.append(f"{svc} depende de {tgt}, sin manifiesto conocido")
            elif d["method"] not in known[tgt].get("methods", {}):
                errors.append(f"{svc} llama {tgt}.{d['method']}, que {tgt} no expone")
    for ev, (owner, _) in emitters.items():
        if not any(ev in m.get("consumes", {}) for m in manifests):
            warnings.append(f"{ev} ({owner}) no tiene consumidores")
    return errors, warnings

# ---------- cli ----------

def main(argv=None):
    p = argparse.ArgumentParser(prog="axon")
    sub = p.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build", help="manifiesto -> codigo")
    b.add_argument("manifest"); b.add_argument("--lang", default="ts", choices=BUILDERS)
    for name, helptext in [("infra", "manifiestos -> terraform"),
                           ("graph", "manifiestos -> mermaid"),
                           ("verify", "detecta drift entre manifiestos")]:
        s = sub.add_parser(name, help=helptext); s.add_argument("sources", nargs="+")
    d = sub.add_parser("discover", help="descubre servicios (dir, archivo o URL)")
    d.add_argument("sources", nargs="+")

    a = p.parse_args(argv)
    if a.cmd == "build":
        print(BUILDERS[a.lang](load(a.manifest))); return 0

    ms = discover(a.sources)
    if a.cmd == "infra":
        print(build_infra(ms))
    elif a.cmd == "graph":
        print(build_graph(ms))
    elif a.cmd == "discover":
        print(json.dumps(registry(ms), indent=2))
    elif a.cmd == "verify":
        errors, warnings = verify(ms)
        for w in warnings: print(f"warn: {w}")
        for e in errors: print(f"error: {e}", file=sys.stderr)
        print(f"axon: {len(ms)} servicios, {len(errors)} errores, {len(warnings)} avisos")
        return 1 if errors else 0
    return 0

if __name__ == "__main__":
    sys.exit(main())
