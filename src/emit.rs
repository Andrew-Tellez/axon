//! Proyecciones del manifiesto: codigo, IaC, CI/CD y diagramas.
use crate::manifest::*;
use indexmap::IndexMap;
use std::collections::BTreeSet;

const ENVELOPE_TS: &str = r#"
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
"#;

fn iface(name: &str, fields: &Fields) -> String {
    let body: String = fields
        .iter()
        .map(|(k, v)| format!("  {k}: {};\n", ts_type(v)))
        .collect();
    format!("export interface {name} {{\n{body}}}\n")
}

/// `all` son los demas manifiestos: el tipo de un evento consumido lo declara
/// su EMISOR, no quien lo recibe. Es la misma razon por la que las fixtures de
/// prueba salen del emisor — ahi es donde aparece el drift.
pub fn build_ts(m: &Manifest, all: &[Manifest]) -> Result<String, String> {
    let svc = &m.service;
    let mut out = vec![
        format!(
            "// generado por axon desde {} — no editar\n",
            m.origin
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        ),
        ENVELOPE_TS.to_string(),
    ];
    for (ev, fields) in &m.emits {
        out.push(iface(&pascal(ev), fields));
    }
    for ev in m.consumes.keys() {
        if m.emits.contains_key(ev) {
            continue;
        }
        match all
            .iter()
            .find_map(|o| o.emits.get(ev).map(|f| (&o.service, f)))
        {
            Some((owner, fields)) => {
                out.push(format!("// {ev}: esquema declarado por {owner}, su dueno"));
                out.push(iface(&pascal(ev), fields));
            }
            None => {
                return Err(format!(
                    "{}: consume `{ev}` y no se encontro quien lo emite. Pasa los demas \
                     manifiestos: `axon build {} manifests/`",
                    m.service,
                    m.origin.display()
                ))
            }
        }
    }
    for (meth, spec) in &m.methods {
        out.push(iface(&format!("{}In", pascal(meth)), &spec.input));
        out.push(iface(&format!("{}Out", pascal(meth)), &spec.output));
    }
    out.push(format!(
        "export const manifest = {} as const;\n",
        serde_json::to_string_pretty(m).unwrap_or_default()
    ));

    // Campos explicitos, no parameter properties: son TS puro y no sobreviven
    // al type-stripping de Node ni a un port directo a otro lenguaje.
    let ctor = if m.patterns.outbox {
        "  protected readonly bus: Bus;\n  protected readonly inbox: Inbox;\n  protected readonly outbox: Outbox;\n  constructor(bus: Bus, inbox: Inbox, outbox: Outbox) {\n    this.bus = bus;\n    this.inbox = inbox;\n    this.outbox = outbox;\n  }"
    } else {
        "  protected readonly bus: Bus;\n  protected readonly inbox: Inbox;\n  constructor(bus: Bus, inbox: Inbox) {\n    this.bus = bus;\n    this.inbox = inbox;\n  }"
    };
    let sink = if m.patterns.outbox {
        "this.outbox.stage"
    } else {
        "this.bus.publish"
    };

    let mut cls = vec![
        format!("export abstract class {}Service {{", pascal(svc)),
        ctor.to_string(),
        "  static readonly wellKnown = \"/.well-known/axon.json\";".to_string(),
    ];
    for ev in m.emits.keys() {
        cls.push(format!(
            "  protected {}(data: {}, cause?: Envelope<unknown>) {{",
            camel(&format!("emit.{ev}")),
            pascal(ev)
        ));
        cls.push(format!(
            "    return {sink}(newEnvelope(\"{ev}\", \"{svc}\", data, cause));"
        ));
        cls.push("  }".to_string());
    }
    for (ev, spec) in &m.consumes {
        cls.push(format!("  /** consume {ev} */"));
        cls.push(format!(
            "  abstract {}(e: Envelope<{}>): Promise<void>;",
            spec.handler,
            pascal(ev)
        ));
    }
    for meth in m.methods.keys() {
        cls.push(format!(
            "  abstract {}(input: {p}In, e: Envelope<unknown>): Promise<{p}Out>;",
            camel(meth),
            p = pascal(meth)
        ));
    }
    if !m.consumes.is_empty() {
        cls.push(
            "  /** Punto de entrada unico: rutea por tipo y deduplica por id de envelope. */"
                .into(),
        );
        cls.push("  dispatch(e: Envelope<unknown>): Promise<void> {".into());
        cls.push("    return this.inbox.once(e.id, async () => {".into());
        cls.push("      switch (e.type) {".into());
        for (ev, spec) in &m.consumes {
            cls.push(format!(
                "        case \"{ev}\": return this.{}(e as Envelope<{}>);",
                spec.handler,
                pascal(ev)
            ));
        }
        cls.push(format!(
            "        default: throw new Error(`{svc}: tipo no declarado en el manifiesto: ${{e.type}}`);"
        ));
        cls.push("      }\n    });\n  }".into());
    }
    cls.push("}\n".into());
    out.push(cls.join("\n"));
    if !m.machine.is_empty() {
        out.push(machines_ts(m));
    }
    let rutas: Vec<String> = m
        .methods
        .values()
        .filter_map(|me| me.http.clone())
        .map(|h| format!("\"{h}\""))
        .collect();
    if !rutas.is_empty() {
        // Una ruta declarada que nadie sirve devuelve 404 en produccion y no
        // aparece en ningun test. El runtime puede negarse a arrancar, pero
        // solo si sabe cuales tenia que haber.
        out.push(format!(
            "\n/** Rutas HTTP que declara el manifiesto. El arranque debe fallar si\n \
             *  alguna no tiene handler: un 404 en produccion no avisa a nadie. */\n\
             export const rutasHttp = [{}] as const;\n",
            rutas.join(", ")
        ));
    }
    out.push(format!(
        "\n/** Lado del teorema CAP declarado en el manifiesto: {c}/{p}.\n \
         *  De ahi sale el nivel de aislamiento: pagar dos veces sale mas caro\n \
         *  que reintentar, y servir un dato viejo cuesta menos que no servir. */\n\
         export const nivelAislamiento = \"{a}\" as const;\n{st}",
        c = m.cap.consistency,
        p = m.cap.on_partition,
        a = m.cap.aislamiento(),
        st = m
            .cap
            .max_staleness_ms
            .map(|v| format!(
                "/** Presupuesto de obsolescencia: un dato mas viejo que esto no se sirve. */\n\
                 export const obsolescenciaMaximaMs = {v};\n"
            ))
            .unwrap_or_default(),
    ));
    out.push(clientes_ts(m, all)?);
    if !m.pii.is_empty() {
        // [A09] Un dato personal se filtra por un log, no por un exploit.
        // El generador da la lista y la funcion; usarla es de la persona,
        // pero no puede alegar que no sabia cuales son.
        out.push(format!(
            "\n/** Campos declarados PII en el manifiesto. */\nexport const camposPII = [{}] as const;\n\n\
             /** Reemplaza todo campo PII por \"[redactado]\", a cualquier profundidad.\n \
             *  Pasa por aqui cualquier objeto antes de mandarlo a un log. */\n\
             export function redactar<T>(valor: T): T {{\n  \
               if (Array.isArray(valor)) return valor.map(redactar) as T;\n  \
               if (valor === null || typeof valor !== \"object\") return valor;\n  \
               const salida: Record<string, unknown> = {{}};\n  \
               for (const [k, v] of Object.entries(valor)) {{\n    \
                 salida[k] = (camposPII as readonly string[]).includes(k) ? \"[redactado]\" : redactar(v);\n  \
               }}\n  return salida as T;\n}}\n",
            m.pii.iter().map(|p| format!("\"{p}\"")).collect::<Vec<_>>().join(", ")
        ));
    }
    Ok(out.join("\n"))
}

// ---------- CI/CD ----------

/// El pipeline tambien es una proyeccion. Los gates los sabe axon; el layout
/// del repo lo dice `axon.policy.toml`, y el despliegue depende del target,
/// igual que la infraestructura. Nada de un cloud hardcodeado.
pub fn build_ci(m: &Manifest, ci: &crate::verify::Ci, target: &str) -> String {
    let svc = &m.service;
    let en = |campo: &String| ci.para(campo, svc);
    let (dir, test, contracts, image, manifests) = (
        en(&ci.service_dir),
        en(&ci.test_cmd),
        en(&ci.contracts_path),
        en(&ci.image),
        ci.manifests_dir.clone(),
    );

    let mut gates = String::new();
    if !migrations_of(m).is_empty() {
        let ruta = m.infra.migrations.clone().unwrap_or_default();
        gates.push_str(&format!(
            "      # gate: expand -> migrate -> contract. Un `.contract.sql` en el mismo
      # deploy que el codigo que deja de usar la columna rompe el rollback.
      - name: migraciones (dry-run)
        run: |
          flyway -url=$DB_URL -locations=filesystem:./{ruta} \\
            -sqlMigrationPrefix= -sqlMigrationSeparator=_ \\
            -validateMigrationNaming=true validate
"
        ));
    }

    let despliegue = match target {
        "gcp" => format!(
            "      - uses: google-github-actions/auth@v2
        with:
          workload_identity_provider: ${{{{ vars.WIF_PROVIDER }}}}
      # la infra va antes que el codigo: el topic tiene que existir cuando
      # arranque el primer pod que publica en el
      - run: axon infra {manifests}/ --target gcp --env prod > infra/generated.tf
      - run: terraform apply -auto-approve
      - run: gcloud run deploy {svc} --image {image} --region ${{{{ vars.REGION }}}}
"
        ),
        "aws" => format!(
            "      - uses: aws-actions/configure-aws-credentials@v4
        with:
          role-to-assume: ${{{{ vars.AWS_ROLE_ARN }}}}
          aws-region: ${{{{ vars.AWS_REGION }}}}
      - run: axon infra {manifests}/ --target aws --env prod > infra/generated.tf
      - run: terraform apply -auto-approve
      - run: |
          aws ecs update-service --cluster ${{{{ vars.ECS_CLUSTER }}}} \\
            --service {svc} --force-new-deployment
"
        ),
        "k8s" => format!(
            "      - run: axon infra {manifests}/ --target k8s --env prod > k8s/generated.yaml
      - run: kubectl apply -f k8s/generated.yaml
      - run: kubectl set image deployment/{svc} {svc}={image}
      - run: kubectl rollout status deployment/{svc} --timeout=5m
"
        ),
        _ => format!(
            "      # Sin target de despliegue. `axon ci --target gcp|aws|k8s` lo genera,
      # o pone aqui el comando de tu plataforma: los gates de arriba son la
      # parte que axon puede saber, esta es la que sabe tu equipo.
      - run: echo \"despliega {svc} aqui\" && exit 1
"
        ),
    };

    format!(
        r#"# generado por axon — no editar
name: {svc}

on:
  pull_request:
    paths: ["{dir}/**", "{manifests}/{svc}.toml"]
  push:
    branches: [main]
    paths: ["{dir}/**", "{manifests}/{svc}.toml"]

concurrency:
  group: {svc}-${{{{ github.ref }}}}
  cancel-in-progress: true

permissions:
  contents: read
  id-token: write   # OIDC: sin llaves de servicio en secrets

jobs:
  contratos:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: curl -fsSL https://raw.githubusercontent.com/Andrew-Tellez/axon/main/install.sh | sh
      # el gate que importa: este manifiesto contra TODOS los demas, y contra
      # los contratos que ya estan publicados
      - run: axon verify {manifests}/
      - name: codigo generado al dia
        run: |
          axon build {manifests}/{svc}.toml {manifests}/ --lang ts > {contracts}
          git diff --exit-code || {{
            echo "::error::codigo generado desactualizado; corre axon build"
            exit 1
          }}
{gates}
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: {test}

  deploy:
    needs: [contratos, test]
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    environment: production
    steps:
      - uses: actions/checkout@v4
      - run: curl -fsSL https://raw.githubusercontent.com/Andrew-Tellez/axon/main/install.sh | sh
      # se despliega por digest, no por etiqueta: una etiqueta es mutable y el
      # deploy deja de ser reproducible y auditable
      - uses: docker/build-push-action@v6
        id: imagen
        with:
          context: {dir}
          push: true
          tags: ${{{{ vars.REGISTRY }}}}/{svc}:${{{{ github.sha }}}}
          provenance: true
{despliegue}      # verificacion contra lo desplegado, no contra el repo
      - run: axon verify https://{svc}.internal
"#
    )
}

// ---------- diagramas ----------

pub fn build_graph(ms: &[Manifest]) -> String {
    let mut out = vec!["graph LR".to_string()];
    for m in ms {
        let n = tfname(&m.service);
        out.push(if m.external {
            format!("  {n}([{}])", m.service)
        } else {
            format!("  {n}[{}]", m.service)
        });
    }
    for m in ms {
        let src = tfname(&m.service);
        for ev in m.emits.keys() {
            out.push(format!("  {src} -- {ev} --> {}(({ev}))", tfname(ev)));
        }
        for ev in m.consumes.keys() {
            out.push(format!("  {}(({ev})) --> {src}", tfname(ev)));
        }
        for d in &m.depends {
            out.push(format!(
                "  {src} -. {} .-> {}",
                d.method,
                tfname(d.target())
            ));
        }
    }
    out.join("\n")
}

pub fn build_classes(ms: &[Manifest]) -> String {
    let mut out = vec!["classDiagram".to_string()];
    for m in ms {
        let cls = format!("{}Service", pascal(&m.service));
        out.push(format!(
            "  class {cls} {{\n    <<{}>>",
            if m.external { "external" } else { "service" }
        ));
        for meth in m.methods.keys() {
            out.push(format!(
                "    +{}({p}In) {p}Out",
                camel(meth),
                p = pascal(meth)
            ));
        }
        for (ev, spec) in &m.consumes {
            out.push(format!(
                "    +{}(Envelope~{}~) void",
                spec.handler,
                pascal(ev)
            ));
        }
        for ev in m.emits.keys() {
            out.push(format!(
                "    #{}({}) void",
                camel(&format!("emit.{ev}")),
                pascal(ev)
            ));
        }
        out.push("  }".into());
        for (ev, fields) in &m.emits {
            out.push(format!("  class {} {{\n    <<event>>", pascal(ev)));
            for (k, v) in fields {
                out.push(format!("    +{k} {v}"));
            }
            out.push("  }".into());
            out.push(format!("  {cls} ..> {} : emits", pascal(ev)));
        }
        for ev in m.consumes.keys() {
            out.push(format!("  {cls} ..> {} : consumes", pascal(ev)));
        }
        for d in &m.depends {
            out.push(format!(
                "  {cls} --> {}Service : {}",
                pascal(d.target()),
                d.method
            ));
        }
        if m.patterns.outbox {
            out.push(format!("  {cls} ..|> Outbox"));
        }
        if !m.consumes.is_empty() {
            out.push(format!("  {cls} ..|> Inbox"));
        }
    }
    out.join("\n")
}

pub fn build_er(ms: &[Manifest]) -> String {
    let mut out = vec!["erDiagram".to_string()];
    for (svc, tables) in schemas(ms) {
        out.push(format!("  %% servicio: {svc}"));
        for (t, cols) in &tables {
            for c in cols {
                if let Some(fk) = &c.fk {
                    out.push(format!(
                        "  {} ||--o{{ {} : {}",
                        fk.to_uppercase(),
                        t.to_uppercase(),
                        c.name
                    ));
                }
            }
        }
        for (t, cols) in &tables {
            out.push(format!("  {} {{", t.to_uppercase()));
            for c in cols {
                let tag = if c.pk {
                    " PK"
                } else if c.fk.is_some() {
                    " FK"
                } else {
                    ""
                };
                out.push(format!("    {} {}{tag}", c.ty, c.name));
            }
            out.push("  }".into());
        }
    }
    out.join("\n")
}

/// Flujo causal esperado. Lo que DEBERIA pasar; el causationId de los
/// envelopes reales dice lo que paso.
pub fn build_seq(ms: &[Manifest], root: &str, solo_eventos: bool) -> Result<String, String> {
    let emitter: IndexMap<&str, &str> = ms
        .iter()
        .flat_map(|m| {
            m.emits
                .keys()
                .map(move |ev| (ev.as_str(), m.service.as_str()))
        })
        .collect();
    if !emitter.contains_key(root) {
        let known: Vec<_> = emitter.keys().copied().collect();
        return Err(format!(
            "{root}: nadie lo emite. Eventos: {}",
            known.join(", ")
        ));
    }
    let external: IndexMap<&str, bool> = ms
        .iter()
        .map(|m| (m.service.as_str(), m.external))
        .collect();
    let mut out = vec!["sequenceDiagram".to_string(), "  autonumber".to_string()];
    let mut seen = BTreeSet::new();
    if solo_eventos {
        // Misma forma que `axon trace --seq`: la cadena causal de eventos, sin
        // las llamadas sincronas, que la traza de envelopes no puede ver.
        out.push(format!("  cliente->>{}: {root}", emitter[root]));
        eventos(ms, &emitter, root, 0, &mut seen, &mut out);
    } else {
        walk(ms, &emitter, &external, root, 0, &mut seen, &mut out);
    }
    Ok(out.join("\n"))
}

fn eventos(
    ms: &[Manifest],
    emitter: &IndexMap<&str, &str>,
    ev: &str,
    depth: u32,
    seen: &mut BTreeSet<(String, u32)>,
    out: &mut Vec<String>,
) {
    if depth > 8 || !seen.insert((ev.to_string(), depth)) {
        return;
    }
    let src = emitter.get(ev).copied().unwrap_or("?");
    for m in ms {
        if !m.consumes.contains_key(ev) {
            continue;
        }
        for nxt in m.emits.keys() {
            out.push(format!("  {src}->>{}: {nxt}", m.service));
            eventos(ms, emitter, nxt, depth + 1, seen, out);
        }
    }
}

fn walk(
    ms: &[Manifest],
    emitter: &IndexMap<&str, &str>,
    external: &IndexMap<&str, bool>,
    ev: &str,
    depth: u32,
    seen: &mut BTreeSet<(String, u32)>,
    out: &mut Vec<String>,
) {
    if depth > 8 || !seen.insert((ev.to_string(), depth)) {
        return;
    }
    let src = emitter.get(ev).copied().unwrap_or("?");
    for m in ms {
        let Some(spec) = m.consumes.get(ev) else {
            continue;
        };
        let dst = &m.service;
        out.push(format!("  {src}->>{dst}: {ev}"));
        out.push(format!("  activate {dst}"));
        for d in &m.depends {
            if d.via.as_deref().is_some_and(|v| v != spec.handler) {
                continue;
            }
            let tgt = d.target();
            let tag = if *external.get(tgt).unwrap_or(&false) {
                " (externo)"
            } else {
                ""
            };
            out.push(format!("  {dst}->>{tgt}: {}{tag}", d.method));
            out.push(format!("  {tgt}-->>{dst}: respuesta"));
        }
        for nxt in m.emits.keys() {
            out.push(format!(
                "  Note over {dst}: emite {nxt} (causationId = id de {ev})"
            ));
            walk(ms, emitter, external, nxt, depth + 1, seen, out);
        }
        out.push(format!("  deactivate {dst}"));
    }
}

// ---------- maquinas de estado ----------

/// Tabla de transiciones exhaustiva y tipada. Un estado ilegal no compila
/// donde el lenguaje lo permite, y falla ruidosamente donde no.
pub fn machines_ts(m: &Manifest) -> String {
    let mut o = Vec::new();
    for (name, mac) in &m.machine {
        let p = pascal(name);
        let states: Vec<String> = mac.states().iter().map(|s| format!("\"{s}\"")).collect();
        let actions: Vec<String> = mac.transitions.keys().map(|a| format!("\"{a}\"")).collect();
        o.push(format!("export type {p}State = {};", states.join(" | ")));
        o.push(format!("export type {p}Action = {};", actions.join(" | ")));
        o.push(format!(
            "export const {}Final: readonly {p}State[] = [{}];",
            camel(name),
            mac.final_states
                .iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        o.push(format!(
            "/** Transiciones declaradas en el manifiesto. Generado: no editar. */\n\
             export const {}Transitions: Record<{p}Action, {{ from: readonly {p}State[]; to: {p}State; on: string }}> = {{",
            camel(name)
        ));
        for (act, t) in &mac.transitions {
            o.push(format!(
                "  {act}: {{ from: [{}], to: \"{}\", on: \"{}\" }},",
                t.from
                    .iter()
                    .map(|f| format!("\"{f}\""))
                    .collect::<Vec<_>>()
                    .join(", "),
                t.to,
                t.on
            ));
        }
        o.push("};".into());
        o.push(format!(
            "export function {c}Next(state: {p}State, action: {p}Action): {p}State {{\n  \
             const t = {c}Transitions[action];\n  \
             if (!t.from.includes(state)) throw new Error(`{name}: ${{action}} no es legal desde ${{state}}`);\n  \
             return t.to;\n}}",
            c = camel(name)
        ));
        o.push(format!(
            "export const {c}Can = (state: {p}State, action: {p}Action) => {c}Transitions[action].from.includes(state);",
            c = camel(name)
        ));
    }
    o.join("\n")
}

pub fn build_states(ms: &[Manifest]) -> String {
    let mut o = vec!["stateDiagram-v2".to_string()];
    for m in ms {
        for (name, mac) in &m.machine {
            o.push(format!("  state \"{}·{name}\" as {name} {{", m.service));
            o.push(format!("    [*] --> {}", mac.initial));
            for (act, t) in &mac.transitions {
                for f in &t.from {
                    o.push(format!("    {f} --> {}: {act}", t.to));
                }
            }
            for f in &mac.final_states {
                o.push(format!("    {f} --> [*]"));
            }
            o.push("  }".into());
        }
    }
    o.join("\n")
}

// ---------- clientes resilientes ----------

const RESILIENCIA_TS: &str = r#"
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
"#;

fn clientes_ts(m: &Manifest, all: &[Manifest]) -> Result<String, String> {
    if m.depends.is_empty() {
        return Ok(String::new());
    }
    let mut tipos = Vec::new();
    let mut metodos = Vec::new();
    for d in &m.depends {
        let tgt = d.target();
        let otro = all
            .iter()
            .find(|o| o.service == tgt)
            .ok_or_else(|| format!("{}: depende de {tgt}, sin manifiesto", m.service))?;
        let firma = otro
            .methods
            .get(&d.method)
            .ok_or_else(|| format!("{}: {tgt} no expone `{}`", m.service, d.method))?;
        // prefijado con el servicio: los tipos de otro no pueden chocar con los propios
        let base = format!("{}{}", pascal(tgt), pascal(&d.method));
        tipos.push(iface(&format!("{base}In"), &firma.input));
        tipos.push(iface(&format!("{base}Out"), &firma.output));

        let pol = format!(
            "{{ timeoutMs: {}, reintentos: {}, breaker: {} }}",
            d.timeout_ms.unwrap_or(10_000),
            d.retries,
            d.breaker
        );
        // `on_partition = "degrade"` obliga a pasar el camino degradado: no se
        // puede llamar al cliente sin decir que se sirve cuando el otro no esta.
        let (param, cuerpo) = if m.cap.degrada() {
            (
                format!(", respaldo: () => Promise<{base}Out>"),
                concat!(
                    "    try {\n",
                    "      return await hacer();\n",
                    "    } catch {\n",
                    "      // declarado `degrade`: se sirve algo viejo antes que nada\n",
                    "      return respaldo();\n",
                    "    }"
                )
                .to_string(),
            )
        } else {
            (String::new(), "    return hacer();".to_string())
        };
        metodos.push(format!(
            "  /** {tgt}.{met} · timeout {t}ms · {r} reintentos · breaker {b} */\n  \
             async {nombre}(input: {base}In, e: Envelope<unknown>{param}): Promise<{base}Out> {{\n    \
               const hacer = () => conPolitica(\"{tgt}.{met}\", {pol}, async () =>\n      \
                 (await this.transporte.invocar(\"{tgt}\", \"{met}\", input, cabeceras(e, {idem}))) as {base}Out);\n\
{cuerpo}\n  \
             }}",
            met = d.method,
            t = d.timeout_ms.unwrap_or(10_000),
            r = d.retries,
            b = d.breaker,
            nombre = camel(&format!("{tgt}.{}", d.method)),
            idem = firma.is_idempotent(),
        ));
    }
    Ok(format!(
        "{}\n{}\n\n/** Clientes de las dependencias declaradas en el manifiesto. */\n\
         export class Clientes {{\n  \
           protected readonly transporte: Transporte;\n  \
           constructor(transporte: Transporte) {{\n    this.transporte = transporte;\n  }}\n\
         {}\n}}\n",
        RESILIENCIA_TS,
        tipos.join("\n"),
        metodos.join("\n")
    ))
}
