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
    if !m.saga.is_empty() {
        out.push(sagas_ts(m));
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
    out.push(flags_ts(m));
    out.push(clientes_ts(m, all)?);
    if !m.pii.is_empty() {
        // [A09] Un dato personal se filtra por un log, no por un exploit.
        // El generador da la lista y la funcion; usarla es de la persona,
        // pero no puede alegar que no sabia cuales son.
        out.push(format!(
            "\n/** Campos declarados PII en el manifiesto. */\nexport const camposPII = [{}] as const;\n\n\
             /** Reemplaza todo campo PII por \"[redactado]\", a cualquier profundidad.\n \
             *  Pasa por aqui cualquier objeto antes de mandarlo a un log.\n \
             *\n \
             *  La comparacion normaliza: `customer_email` declarado en el manifiesto\n \
             *  cubre `customerEmail` en el contrato y `customer-email` en una\n \
             *  cabecera. El mismo concepto se declara una vez. */\n\
             const normalizarPII = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, \"\");\n\
             const pii = new Set((camposPII as readonly string[]).map(normalizarPII));\n\n\
             export function redactar<T>(valor: T): T {{\n  \
               if (Array.isArray(valor)) return valor.map(redactar) as T;\n  \
               if (valor === null || typeof valor !== \"object\") return valor;\n  \
               const salida: Record<string, unknown> = {{}};\n  \
               for (const [k, v] of Object.entries(valor)) {{\n    \
                 salida[k] = pii.has(normalizarPII(k)) ? \"[redactado]\" : redactar(v);\n  \
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
            for c in &cols.cols {
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
            for c in &cols.cols {
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

/// La saga como diagrama: el camino de ida y, en la misma imagen, la vuelta.
///
/// Que la compensacion se dibuje sola es la mitad del valor de declararla: en
/// una revision, un paso sin flecha de vuelta se ve.
fn seq_saga(m: &Manifest, nombre: &str, sg: &Saga) -> String {
    let mut o = vec![
        "sequenceDiagram".to_string(),
        "  autonumber".to_string(),
        format!("  participant coord as {}·{nombre}", m.service),
    ];
    let mut vistos: Vec<&str> = Vec::new();
    for paso in &sg.steps {
        for r in [Some(&paso.hacer), paso.undo.as_ref()].into_iter().flatten() {
            if let Some((svc, _)) = Paso::partes(r) {
                if svc != m.service && !vistos.contains(&svc) {
                    vistos.push(svc);
                    o.push(format!("  participant {svc}"));
                }
            }
        }
    }
    o.push(format!(
        "  Note over coord: presupuesto {}",
        match sg.timeout_ms {
            Some(ms) => format!("{ms}ms"),
            None => "sin declarar".to_string(),
        }
    ));
    for (i, paso) in sg.steps.iter().enumerate() {
        if let Some((svc, met)) = Paso::partes(&paso.hacer) {
            o.push(format!("  coord->>{svc}: {} {met}", i + 1));
            o.push(format!("  {svc}-->>coord: ok"));
        }
    }
    o.push("  Note over coord: hasta aca, el camino feliz".to_string());
    // la vuelta: en orden inverso, que es el unico correcto
    o.push("  rect rgba(200,80,80,0.12)".to_string());
    o.push("  Note over coord: si un paso falla, se deshace lo intentado en orden INVERSO".into());
    for (i, paso) in sg.steps.iter().enumerate().rev() {
        match &paso.undo {
            Some(u) => {
                if let Some((svc, met)) = Paso::partes(u) {
                    o.push(format!("  coord->>{svc}: deshacer {} · {met}", i + 1));
                    o.push(format!("  {svc}-->>coord: ok (idempotente)"));
                }
            }
            None => o.push(format!(
                "  Note over coord: paso {} sin compensacion: es el ultimo",
                i + 1
            )),
        }
    }
    o.push("  end".to_string());
    o.join("\n")
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
    // Una saga tambien es un flujo, y el suyo tiene una rama que ningun
    // diagrama de eventos muestra: la compensacion.
    if let Some((m, sg)) = ms
        .iter()
        .find_map(|m| m.saga.get(root).map(|sg| (m, sg)))
    {
        return Ok(seq_saga(m, root, sg));
    }
    if !emitter.contains_key(root) {
        let known: Vec<_> = emitter.keys().copied().collect();
        let sagas: Vec<&str> = ms
            .iter()
            .flat_map(|m| m.saga.keys().map(|k| k.as_str()))
            .collect();
        return Err(format!(
            "{root}: nadie lo emite y ninguna saga se llama asi. Eventos: {}. Sagas: {}",
            known.join(", "),
            if sagas.is_empty() {
                "ninguna".to_string()
            } else {
                sagas.join(", ")
            }
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

/// El coordinador de cada saga.
///
/// Lo que se genera es el CAMINO: el orden de los pasos, el diario, la
/// compensacion en orden inverso y el presupuesto de tiempo. Lo que no se
/// genera son las entradas de cada llamada, porque son datos de negocio: eso
/// es la interfaz que se implementa. Asi que un paso sin implementar no
/// compila, y un paso implementado no puede correr fuera de orden.
pub fn sagas_ts(m: &Manifest) -> String {
    let mut o = vec![
        "\n/** El diario de una saga: donde vive el avance. Sin el, un reinicio a\n \
         *  mitad de camino deja los pasos ya hechos aplicados y sin registro de\n \
         *  cuales fueron: no se puede terminar ni compensar.\n \
         *\n \
         *  `intentando` se escribe ANTES de la llamada y `hecho` DESPUES. Un paso\n \
         *  que quedo en `intentando` puede haber ocurrido o no, asi que al retomar\n \
         *  se COMPENSA, no se reintenta: por eso toda compensacion tiene que\n \
         *  tolerar que no haya nada que deshacer.\n \
         *\n \
         *  Guarda tambien el envelope que la arranco. Retomar sin el es imposible:\n \
         *  las acciones necesitan los datos de la llamada, y el proceso que los\n \
         *  tenia en memoria es justo el que se murio. */\n\
         export interface SagaDiario {\n  \
           abrir(id: string, saga: string, e: Envelope<unknown>): Promise<void>;\n  \
           marcar(id: string, paso: number, estado: \"intentando\" | \"hecho\" | \"deshecho\"): Promise<void>;\n  \
           cerrar(id: string, estado: SagaEstado): Promise<void>;\n  \
           /** Hasta donde llego, para retomar. `null` si es nueva. */\n  \
           leer(id: string): Promise<{ paso: number; estado: string } | null>;\n  \
           /** Reclama hasta `limite` sagas abiertas que no avanzan desde `antesDe`,\n   \
            *  y devuelve el envelope de cada una.\n   \
            *\n   \
            *  RECLAMA, no lista: dos instancias del servicio barren a la vez, y dos\n   \
            *  coordinadores sobre la misma saga compensan los mismos pasos dos\n   \
            *  veces. En Postgres el reclamo y el filtro son la misma sentencia:\n   \
            *\n   \
            *    UPDATE saga_<nombre> SET actualizado = now()\n   \
            *     WHERE estado IN ('intentando','hecho') AND actualizado < $1\n   \
            *     RETURNING id, datos\n   \
            *    LIMIT $2\n   \
            *\n   \
            *  Tocar `actualizado` es el reclamo: el otro barredor ya no la ve. Y si\n   \
            *  este proceso muere a mitad, vuelve a ser elegible en la siguiente\n   \
            *  ventana sin que nadie la desbloquee a mano. */\n  \
           reclamar(saga: string, antesDe: Date, limite: number): Promise<{ id: string; datos: Envelope<unknown> }[]>;\n\
         }\n\
         \n\
         export type SagaEstado = \"completada\" | \"compensada\" | \"atascada\";\n\
         \n\
         /** Una compensacion que falla no tiene nada detras: la saga queda a\n \
         *  medias y necesita una persona. Se lanza para que eso no pase\n \
         *  desapercibido. */\n\
         export class SagaAtascada extends Error {\n  \
           readonly saga: string;\n  \
           readonly paso: number;\n  \
           readonly causa: unknown;\n  \
           constructor(saga: string, paso: number, causa: unknown) {\n    \
             super(`${saga}: la compensacion del paso ${paso} fallo; la saga quedo a medias`);\n    \
             this.saga = saga;\n    \
             this.paso = paso;\n    \
             this.causa = causa;\n  \
           }\n\
         }\n\
         \n\
         /** Lo que hizo una pasada del barrido. Se devuelve para que se pueda\n \
         *  medir: un barrido que no reporta nada es indistinguible de uno que no\n \
         *  corre. */\n\
         export interface SagaBarrido {\n  \
           reclamadas: number;\n  \
           completadas: number;\n  \
           compensadas: number;\n  \
           /** Necesitan una persona. El barrido NO las reintenta. */\n  \
           atascadas: number;\n  \
           /** Quedaron para la proxima pasada porque se alcanzo el limite. */\n  \
           pendientes: boolean;\n\
         }\n"
            .to_string(),
    ];
    for (nombre, sg) in &m.saga {
        let p = pascal(nombre);
        let c = camel(nombre);
        let mut acciones = Vec::new();
        let mut tabla = Vec::new();
        for (i, paso) in sg.steps.iter().enumerate() {
            let n = i + 1;
            let met = Paso::partes(&paso.hacer).map(|(_, x)| x).unwrap_or("?");
            acciones.push(format!(
                "  /** paso {n} · {} */\n  paso{n}{}(e: Envelope<unknown>): Promise<void>;",
                paso.hacer,
                pascal(met)
            ));
            match &paso.undo {
                Some(u) => {
                    let umet = Paso::partes(u).map(|(_, x)| x).unwrap_or("?");
                    acciones.push(format!(
                        "  /** deshace el paso {n} · {u} · tiene que tolerar que no haya nada que deshacer */\n  \
                         deshacer{n}{}(e: Envelope<unknown>): Promise<void>;",
                        pascal(umet)
                    ));
                    tabla.push(format!(
                        "  {{ paso: {n}, hacer: \"{}\", deshacer: \"{u}\" }},",
                        paso.hacer
                    ));
                }
                None => tabla.push(format!(
                    "  // el ultimo paso no lleva compensacion: si falla, no hay nada suyo que deshacer\n  \
                     {{ paso: {n}, hacer: \"{}\", deshacer: null }},",
                    paso.hacer
                )),
            }
        }
        o.push(format!(
            "/** La ruta que golpea el programador para correr una pasada del\n \
             *  barrido. `axon infra` la despliega en los cuatro targets, asi que el\n \
             *  arranque tiene que servirla llamando a `barrer{p}`: un programador\n \
             *  apuntando a un 404 se aplica sin error y no barre nada.\n \
             *\n \
             *  NO es un metodo declarado, asi que no sale por el gateway. Dispara\n \
             *  compensaciones: no puede ser publica. */\n\
             export const rutaBarrido{p} = \"POST /internal/saga/{nombre}/barrer\" as const;\n"
        ));
        o.push(format!(
            "/** Los pasos declarados en el manifiesto. Generado: no editar. */\n\
             export const {c}Pasos = [\n{}\n] as const;\n",
            tabla.join("\n")
        ));
        o.push(format!(
            "/** Un metodo por paso y uno por compensacion. Los implementa quien\n \
             *  conoce los datos: el coordinador sabe el orden, no el contenido. */\n\
             export interface {p}Acciones {{\n{}\n}}\n",
            acciones.join("\n")
        ));

        // el cuerpo: hacia adelante hasta que algo falla, y de vuelta
        let mut adelante = Vec::new();
        let mut atras = Vec::new();
        for (i, paso) in sg.steps.iter().enumerate() {
            let n = i + 1;
            let met = Paso::partes(&paso.hacer).map(|(_, x)| x).unwrap_or("?");
            adelante.push(format!(
                "      case {n}:\n        \
                   await diario.marcar(id, {n}, \"intentando\");\n        \
                   await acciones.paso{n}{}(e);\n        \
                   await diario.marcar(id, {n}, \"hecho\");\n        \
                   break;",
                pascal(met)
            ));
            if let Some(u) = &paso.undo {
                let umet = Paso::partes(u).map(|(_, x)| x).unwrap_or("?");
                atras.push(format!(
                    "      case {n}:\n        \
                       await acciones.deshacer{n}{}(e);\n        \
                       break;",
                    pascal(umet)
                ));
            } else {
                atras.push(format!(
                    "      case {n}:\n        \
                       break; // sin compensacion declarada: es el ultimo paso"
                ));
            }
        }
        let plazo = match sg.timeout_ms {
            Some(ms) => format!("{ms}"),
            None => "Number.POSITIVE_INFINITY".to_string(),
        };
        o.push(format!(
            "/** Corre la saga `{nombre}`.\n \
             *\n \
             *  Hacia adelante hasta que un paso falla o se agota el presupuesto; de\n \
             *  ahi en orden INVERSO deshaciendo solo lo que se intento. El orden\n \
             *  inverso no es estetica: compensar hacia adelante deshace un paso\n \
             *  cuyo efecto otro paso posterior ya uso.\n \
             *\n \
             *  Si `id` ya tiene diario, retoma: el paso que quedo en `intentando`\n \
             *  se compensa, porque no se sabe si ocurrio. */\n\
             export async function correr{p}(\n  \
               id: string,\n  \
               acciones: {p}Acciones,\n  \
               diario: SagaDiario,\n  \
               e: Envelope<unknown>,\n\
             ): Promise<{{ estado: SagaEstado; hasta: number; error?: unknown }}> {{\n  \
               const total = {total};\n  \
               // presupuesto declarado en el manifiesto; `axon verify` ya comprobo\n  \
               // que cubre la suma de los pasos y sus compensaciones\n  \
               const limite = Date.now() + {plazo};\n  \
               const previo = await diario.leer(id);\n  \
               if (!previo) await diario.abrir(id, \"{nombre}\", e);\n  \
               // un paso a medio intentar no se reintenta: se deshace\n  \
               let hecho = previo ? (previo.estado === \"hecho\" ? previo.paso : previo.paso - 1) : 0;\n  \
               const dudoso = previo?.estado === \"intentando\" ? previo.paso : 0;\n  \
               // El paso que FALLO tambien se deshace: un timeout no dice que no\n  \
               // paso nada del otro lado. Compensar solo hasta el ultimo exito\n  \
               // deja ese efecto aplicado para siempre.\n  \
               let intentado = dudoso;\n  \
               let fallo: unknown = dudoso ? new Error(\"retomada con un paso en duda\") : undefined;\n  \
               if (!dudoso) {{\n    \
                 for (let paso = hecho + 1; paso <= total; paso++) {{\n      \
                   if (Date.now() > limite) {{\n        \
                     fallo = new Error(`{nombre}: presupuesto agotado antes del paso ${{paso}}`);\n        \
                     break;\n      \
                   }}\n      \
                   intentado = paso;\n      \
                   try {{\n        \
                     await paso{p}(paso, acciones, diario, e, id);\n        \
                     hecho = paso;\n      \
                   }} catch (err) {{\n        \
                     fallo = err;\n        \
                     break;\n      \
                   }}\n    \
                 }}\n  \
               }}\n  \
               if (!fallo) {{\n    \
                 await diario.cerrar(id, \"completada\");\n    \
                 return {{ estado: \"completada\", hasta: total }};\n  \
               }}\n  \
               // de vuelta: todo lo que se INTENTO, en orden inverso\n  \
               for (let paso = intentado; paso >= 1; paso--) {{\n    \
                 try {{\n      \
                   await deshacer{p}(paso, acciones, e);\n      \
                   await diario.marcar(id, paso, \"deshecho\");\n    \
                 }} catch (err) {{\n      \
                   await diario.cerrar(id, \"atascada\");\n      \
                   throw new SagaAtascada(\"{nombre}\", paso, err);\n    \
                 }}\n  \
               }}\n  \
               await diario.cerrar(id, \"compensada\");\n  \
               return {{ estado: \"compensada\", hasta: hecho, error: fallo }};\n\
             }}\n\
             \n\
             async function paso{p}(\n  \
               paso: number,\n  \
               acciones: {p}Acciones,\n  \
               diario: SagaDiario,\n  \
               e: Envelope<unknown>,\n  \
               id: string,\n\
             ): Promise<void> {{\n  \
               switch (paso) {{\n\
             {adelante}\n    \
                 default:\n      \
                   throw new Error(`{nombre}: paso ${{paso}} no declarado en el manifiesto`);\n  \
               }}\n\
             }}\n\
             \n\
             async function deshacer{p}(paso: number, acciones: {p}Acciones, e: Envelope<unknown>): Promise<void> {{\n  \
               switch (paso) {{\n\
             {atras}\n    \
                 default:\n      \
                   throw new Error(`{nombre}: paso ${{paso}} no declarado en el manifiesto`);\n  \
               }}\n\
             }}\n",
            total = sg.steps.len(),
            adelante = adelante.join("\n"),
            atras = atras.join("\n"),
        ));

        // El barrido: quien vuelve a llamar a la saga que quedo colgada.
        //
        // El coordinador ya sabe retomar, pero nadie lo llama: el proceso que
        // la tenia en vuelo es el que se murio. Sin esto, una saga con un paso
        // en `intentando` se queda ahi para siempre, y el diario la registra sin
        // que nadie lo lea.
        let ventana = sg.timeout_ms.unwrap_or(60_000);
        o.push(format!(
            "/** Una pasada del barrido: retoma las sagas `{nombre}` que no avanzan.\n \
             *\n \
             *  Solo toca las que llevan mas de su PRESUPUESTO sin moverse ({ventana}ms).\n \
             *  Ese umbral no es una heuristica: `axon verify` ya comprobo que el\n \
             *  presupuesto cubre la suma de los pasos y sus compensaciones, asi que\n \
             *  una saga mas vieja que eso no esta en camino, esta colgada. Barrer\n \
             *  antes seria correr un segundo coordinador sobre una saga viva.\n \
             *\n \
             *  Una que quedo `atascada` NO se reintenta: se cuenta y se deja. Una\n \
             *  compensacion que ya fallo necesita una persona, y reintentarla en\n \
             *  silencio esconde justo eso. */\n\
             export async function barrer{p}(\n  \
               acciones: {p}Acciones,\n  \
               diario: SagaDiario,\n  \
               limite = 50,\n\
             ): Promise<SagaBarrido> {{\n  \
               const antesDe = new Date(Date.now() - {ventana});\n  \
               const colgadas = await diario.reclamar(\"{nombre}\", antesDe, limite);\n  \
               const r: SagaBarrido = {{\n    \
                 reclamadas: colgadas.length,\n    \
                 completadas: 0,\n    \
                 compensadas: 0,\n    \
                 atascadas: 0,\n    \
                 // si se lleno el limite hay mas esperando, y decirlo es la\n    \
                 // diferencia entre un barrido que va al dia y uno que no alcanza\n    \
                 pendientes: colgadas.length >= limite,\n  \
               }};\n  \
               for (const {{ id, datos }} of colgadas) {{\n    \
                 try {{\n      \
                   const salida = await correr{p}(id, acciones, diario, datos);\n      \
                   if (salida.estado === \"completada\") r.completadas++;\n      \
                   else r.compensadas++;\n    \
                 }} catch (err) {{\n      \
                   // una saga atascada no aborta la pasada: las demas siguen\n      \
                   // colgadas y este es el unico que las va a mirar\n      \
                   if (err instanceof SagaAtascada) r.atascadas++;\n      \
                   else throw err;\n    \
                 }}\n  \
               }}\n  \
               return r;\n\
             }}\n\
             \n\
             /** Arranca el barrido periodico y devuelve como pararlo.\n \
             *\n \
             *  El intervalo sale del presupuesto declarado: nada se vuelve elegible\n \
             *  antes, asi que barrer mas seguido es trabajo sin resultado. Es seguro\n \
             *  con varias instancias porque `reclamar` reclama.\n \
             *\n \
             *  `alTerminar` recibe cada pasada. Conectalo a las metricas: un barrido\n \
             *  que no reporta es indistinguible de uno que no corre, y este es el\n \
             *  unico lugar desde donde se ve que una saga quedo atascada. */\n\
             export function arrancarBarrido{p}(\n  \
               acciones: {p}Acciones,\n  \
               diario: SagaDiario,\n  \
               alTerminar: (r: SagaBarrido) => void,\n  \
               intervaloMs = {ventana},\n\
             ): () => void {{\n  \
               let corriendo = false;\n  \
               const t = setInterval(async () => {{\n    \
                 // sin esto, una pasada lenta se solapa con la siguiente en el\n    \
                 // mismo proceso y las dos reclaman\n    \
                 if (corriendo) return;\n    \
                 corriendo = true;\n    \
                 try {{\n      \
                   alTerminar(await barrer{p}(acciones, diario));\n    \
                 }} finally {{\n      \
                   corriendo = false;\n    \
                 }}\n  \
               }}, intervaloMs);\n  \
               // que un barrido de fondo no mantenga el proceso vivo al apagarlo\n  \
               t.unref?.();\n  \
               return () => clearInterval(t);\n\
             }}\n"
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

// ---------- feature flags ----------

/// Accesores tipados y la interfaz del proveedor.
///
/// La interfaz tiene la forma de OpenFeature a proposito: axon no trae un SDK
/// de flags ni inventa un protocolo, igual que no trae uno de trazas. Lo que
/// aporta es que el nombre del flag, su valor seguro y el campo por el que se
/// fija salgan del manifiesto y no de una cadena suelta en el codigo.
fn flags_ts(m: &Manifest) -> String {
    if m.flags.is_empty() {
        return String::new();
    }
    // La interfaz cubre los cuatro tipos de OpenFeature. Un flag no es solo un
    // booleano: el estandar admite string, numero y objeto, y un rollout de
    // configuracion —un limite, un proveedor, un umbral— necesita justamente eso.
    let mut o = vec![
        "\n/** Proveedor de flags, con la forma de OpenFeature: `evaluar` recibe el\n \
         *  nombre, el valor por defecto y el contexto por el que se fija. Los cuatro\n \
         *  tipos del estandar, para que el SDK real encaje sin traduccion. */\n\
         export interface Flags {\n  \
           evaluar<T extends boolean | string | number | object>(\n    \
             nombre: string,\n    porDefecto: T,\n    contexto: Record<string, string>,\n  \
           ): Promise<T>;\n\
         }\n"
        .to_string(),
    ];
    let mut nombres = Vec::new();
    for (nombre, f) in &m.flags {
        nombres.push(format!("\"{nombre}\""));
        let variantes = f.variantes();
        let defecto = f.variante_defecto();
        let valor = variantes
            .get(&defecto)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "false".into());
        let tipo = match f.tipo() {
            "boolean" => "boolean".to_string(),
            "string" => "string".to_string(),
            "number" => "number".to_string(),
            // el tipo del objeto sale del propio valor por defecto declarado
            _ => format!("typeof {}", camel(&format!("valor.{nombre}"))),
        };
        let mut doc = vec![format!("/** `{nombre}`: {} de OpenFeature.", f.tipo())];
        if variantes.len() > 2 || !f.variants.is_empty() {
            doc.push(format!(
                " *  Variantes: {}.",
                variantes
                    .iter()
                    .map(|(k, v)| format!("`{k}` = {v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(c) = &f.sticky_by {
            doc.push(format!(
                " *  Se fija por `{c}`: la misma entidad toma siempre el mismo camino."
            ));
        }
        doc.push(" */".into());

        if f.tipo() == "object" {
            o.push(format!(
                "const {} = {valor} as const;",
                camel(&format!("valor.{nombre}"))
            ));
        }
        let firma = match &f.sticky_by {
            Some(c) => format!("(flags: Flags, {c}: string)"),
            None => "(flags: Flags)".to_string(),
        };
        let contexto = match &f.sticky_by {
            Some(c) => format!("{{ targetingKey: {c}, {c} }}"),
            None => "{}".to_string(),
        };
        let por_defecto = if f.tipo() == "object" {
            camel(&format!("valor.{nombre}"))
        } else {
            valor.clone()
        };
        o.push(format!(
            "{}\nexport const {} = {firma}: Promise<{tipo}> =>\n  \
               flags.evaluar(\"{nombre}\", {por_defecto}, {contexto});\n",
            doc.join("\n"),
            camel(&format!("flag.{nombre}")),
        ));
    }
    o.push(format!(
        "/** Los flags que declara el manifiesto. Un flag que no esta aca no existe. */\n\
         export const flagsDeclarados = [{}] as const;\n",
        nombres.join(", ")
    ));
    o.join("\n")
}

/// Configuracion de flagd, que es la implementacion de referencia de
/// OpenFeature y lee exactamente este JSON. El rollout gradual se expresa con
/// su `fractional`, fijado por el campo declarado en `sticky_by`.
pub fn build_flagd(ms: &[Manifest]) -> String {
    let mut flags = Vec::new();
    for m in ms.iter().filter(|m| !m.external) {
        for (nombre, f) in &m.flags {
            // Las variantes declaradas, no un on/off fijo: OpenFeature admite
            // string, numero y objeto, y flagd los resuelve igual.
            let variantes = format!(
                "{{ {} }}",
                f.variantes()
                    .iter()
                    .map(|(k, v)| format!("\"{k}\": {v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            let por_defecto = f.variante_defecto();
            let objetivo = match (f.rollout, &f.sticky_by) {
                (Some(p), Some(campo)) if p > 0 && p < 100 => {
                    // el rollout reparte entre la variante por defecto y la
                    // otra; con mas de dos variantes hay que declararlo a mano
                    let destino = f
                        .variantes()
                        .keys()
                        .find(|k| **k != por_defecto)
                        .cloned()
                        .unwrap_or_else(|| "on".into());
                    format!(
                        ",\n      \"targeting\": {{\n        \"fractional\": [\n          \
                         {{ \"var\": \"{campo}\" }},\n          [\"{destino}\", {p}],\n          \
                         [\"{por_defecto}\", {}]\n        ]\n      }}",
                        100 - p
                    )
                }
                _ => String::new(),
            };
            flags.push(format!(
                "    \"{}\": {{\n      \"state\": \"ENABLED\",\n      \
                 \"variants\": {variantes},\n      \"defaultVariant\": \"{por_defecto}\"{objetivo}\n    }}",
                nombre
            ));
        }
    }
    format!(
        "{{\n  \"$schema\": \"https://flagd.dev/schema/v0/flags.json\",\n  \"flags\": {{\n{}\n  }}\n}}\n",
        flags.join(",\n")
    )
}
