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

# ---------- classes / er / seq ----------

def build_classes(manifests):
    """Diagrama de clases: proyeccion directa de los manifiestos."""
    out = ["classDiagram"]
    for m in manifests:
        svc, cls = m["service"], pascal(m["service"]) + "Service"
        if m.get("external"):
            out.append(f"  class {cls} {{\n    <<external>>")
        else:
            out.append(f"  class {cls} {{\n    <<service>>")
        for meth, spec in m.get("methods", {}).items():
            out.append(f"    +{camel(meth)}({pascal(meth)}In) {pascal(meth)}Out")
        for ev, spec in m.get("consumes", {}).items():
            out.append(f"    +{spec['handler']}(Envelope~{pascal(ev)}~) void")
        for ev in m.get("emits", {}):
            out.append(f"    #{camel('emit.' + ev)}({pascal(ev)}) void")
        out.append("  }")
        for ev, fields in m.get("emits", {}).items():
            out.append(f"  class {pascal(ev)} {{\n    <<event>>")
            out += [f"    +{k} {v}" for k, v in fields.items()]
            out.append("  }")
            out.append(f"  {cls} ..> {pascal(ev)} : emits")
        for ev in m.get("consumes", {}):
            out.append(f"  {cls} ..> {pascal(ev)} : consumes")
        for d in m.get("depends", []):
            tgt = pascal(d.get("service") or d["external"]) + "Service"
            out.append(f"  {cls} --> {tgt} : {d['method']}")
        if m.get("patterns", {}).get("outbox"):
            out.append(f"  {cls} ..|> Outbox")
        if m.get("consumes"):
            out.append(f"  {cls} ..|> Inbox")
    return "\n".join(out)


# ponytail: regex, no parser SQL. Aguanta CREATE TABLE / REFERENCES normales.
# Si se rompe: `pg_dump --schema-only` es mas regular, y information_schema
# es la salida definitiva (cuesta una dependencia de driver).
CREATE_RE = re.compile(r'create\s+table\s+(?:if\s+not\s+exists\s+)?"?([\w.]+)"?\s*\((.*?)\n\s*\)\s*;', re.I | re.S)
SKIP = ("primary key", "foreign key", "constraint", "unique", "check", "--")

def split_cols(body):
    depth, cur = 0, ""
    for ch in body:
        if ch == "(": depth += 1
        elif ch == ")": depth -= 1
        if ch == "," and depth == 0:
            yield cur; cur = ""
        else:
            cur += ch
    yield cur

def parse_ddl(text):
    tables = {}
    for name, body in CREATE_RE.findall(text):
        cols = []
        for raw in split_cols(body):
            line = raw.strip()
            if not line or line.lower().startswith(SKIP):
                continue
            parts = line.split()
            fk = re.search(r'references\s+"?([\w.]+)"?', line, re.I)
            cols.append({"name": parts[0].strip('"'), "type": parts[1].rstrip(","),
                         "pk": "primary key" in line.lower(),
                         "fk": fk.group(1).strip('"') if fk else None})
        tables[name.strip('"')] = cols
    return tables

ALTER_ADD = re.compile(r'alter\s+table\s+"?([\w.]+)"?\s+add\s+column\s+(?:if\s+not\s+exists\s+)?"?(\w+)"?\s+(\w+)', re.I)
ALTER_DROP = re.compile(r'alter\s+table\s+"?([\w.]+)"?\s+drop\s+column\s+(?:if\s+exists\s+)?"?(\w+)"?', re.I)
DROP_TABLE = re.compile(r'drop\s+table\s+(?:if\s+exists\s+)?"?([\w.]+)"?', re.I)

def migrations_of(m):
    """Archivos de migracion de un servicio, en orden. Fuente de verdad del esquema."""
    path = m.get("infra", {}).get("migrations") or m.get("infra", {}).get("schema")
    if not path:
        return []
    f = Path(path) if Path(path).is_absolute() else Path(m["_path"]).parent / path
    if f.is_dir():
        return sorted(f.glob("*.sql"))
    if not f.exists():
        die(f"{m['service']}: migraciones no encontradas: {f}")
    return [f]

def schemas(manifests):
    """{servicio: {tabla: [columnas]}} plegando las migraciones en orden.
    No hay esquema declarado aparte: el esquema ES la suma de las migraciones."""
    out = {}
    for m in manifests:
        files = migrations_of(m)
        if not files:
            continue
        tables = {}
        for f in files:
            text = f.read_text()
            tables.update(parse_ddl(text))
            for t, col, typ in ALTER_ADD.findall(text):
                tables.setdefault(t.strip('"'), []).append(
                    {"name": col, "type": typ, "pk": False, "fk": None})
            for t, col in ALTER_DROP.findall(text):
                tables[t.strip('"')] = [c for c in tables.get(t.strip('"'), []) if c["name"] != col]
            for t in DROP_TABLE.findall(text):
                tables.pop(t.strip('"'), None)
        out[m["service"]] = tables
    return out

def destructive(text):
    return bool(ALTER_DROP.search(text) or DROP_TABLE.search(text))

def build_er(manifests):
    by_svc = schemas(manifests)
    out = ["erDiagram"]
    for svc, tables in by_svc.items():
        out.append(f"  %% servicio: {svc}")
        for t, cols in tables.items():
            for c in cols:
                if c["fk"]:
                    out.append(f'  {c["fk"].upper()} ||--o{{ {t.upper()} : {c["name"]}')
        for t, cols in tables.items():
            out.append(f"  {t.upper()} {{")
            for c in cols:
                out.append(f'    {c["type"]} {c["name"]}{" PK" if c["pk"] else " FK" if c["fk"] else ""}')
            out.append("  }")
    return "\n".join(out)

def build_seq(manifests, root):
    """Flujo causal esperado a partir de un evento. Lo que DEBERIA pasar;
    el causationId de los envelopes reales dice lo que paso."""
    by_svc = {m["service"]: m for m in manifests}
    emitter = {ev: m["service"] for m in manifests for ev in m.get("emits", {})}
    if root not in emitter:
        die(f"{root}: nadie lo emite. Eventos: {', '.join(sorted(emitter)) or '(ninguno)'}")
    out, seen = ["sequenceDiagram", "  autonumber"], set()

    def walk(ev, depth=0):
        if (ev, depth) in seen or depth > 8:
            return
        seen.add((ev, depth))
        src = emitter.get(ev)
        for m in manifests:
            spec = m.get("consumes", {}).get(ev)
            if not spec:
                continue
            dst, handler = m["service"], spec["handler"]
            out.append(f"  {src}->>{dst}: {ev}")
            out.append(f"  activate {dst}")
            for d in m.get("depends", []):
                if d.get("via") and d["via"] != handler:
                    continue
                tgt = d.get("service") or d["external"]
                tag = " (externo)" if by_svc.get(tgt, {}).get("external") else ""
                out.append(f"  {dst}->>{tgt}: {d['method']}{tag}")
                out.append(f"  {tgt}-->>{dst}: respuesta")
            for nxt in m.get("emits", {}):
                out.append(f"  Note over {dst}: emite {nxt} (causationId = id de {ev})")
                walk(nxt, depth + 1)
            out.append(f"  deactivate {dst}")

    walk(root)
    return "\n".join(out)

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
    for m in manifests:
        for f in migrations_of(m):
            if destructive(f.read_text()) and ".contract." not in f.name:
                errors.append(f"{m['service']}/{f.name}: migracion destructiva sin marcar "
                              f"como `.contract.sql` (expand -> migrate -> contract)")
            if not re.match(r"^\d{3,}_", f.name):
                warnings.append(f"{m['service']}/{f.name}: sin prefijo numerico, el orden no es determinista")
    owner_of = {t: svc for svc, ts in schemas(manifests).items() for t in ts}
    for svc, tables in schemas(manifests).items():
        for t, cols in tables.items():
            for c in cols:
                if c["fk"] and owner_of.get(c["fk"], svc) != svc:
                    errors.append(f"{svc}.{t}.{c['name']} -> {c['fk']}: FK cruza el limite "
                                  f"de servicio (dueno: {owner_of[c['fk']]}); usa el id, no una FK")
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
    q = sub.add_parser("seq", help="flujo causal de un evento -> mermaid sequenceDiagram")
    q.add_argument("event"); q.add_argument("sources", nargs="+")
    for name, helptext in [("infra", "manifiestos -> terraform"),
                           ("graph", "manifiestos -> mermaid (topologia)"),
                           ("classes", "manifiestos -> mermaid classDiagram"),
                           ("er", "DDL declarado en [infra].schema -> mermaid erDiagram"),
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
    elif a.cmd == "classes":
        print(build_classes(ms))
    elif a.cmd == "er":
        print(build_er(ms))
    elif a.cmd == "seq":
        print(build_seq(ms, a.event))
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
