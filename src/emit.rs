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
    // Solo los colaboradores que el servicio USA. Pedirle un bus a un
    // coordinador que no emite nada obliga a fabricar uno de mentira, y un
    // objeto de mentira en el constructor es una dependencia que nadie revisa.
    let mut campos = Vec::new();
    let mut params = Vec::new();
    let mut asigna = Vec::new();
    if !m.emits.is_empty() {
        campos.push("  protected readonly bus: Bus;");
        params.push("bus: Bus");
        asigna.push("    this.bus = bus;");
    }
    if !m.consumes.is_empty() {
        campos.push("  protected readonly inbox: Inbox;");
        params.push("inbox: Inbox");
        asigna.push("    this.inbox = inbox;");
    }
    if m.patterns.outbox {
        campos.push("  protected readonly outbox: Outbox;");
        params.push("outbox: Outbox");
        asigna.push("    this.outbox = outbox;");
    }
    let ctor = if params.is_empty() {
        String::new()
    } else {
        format!(
            "{}\n  constructor({}) {{\n{}\n  }}",
            campos.join("\n"),
            params.join(", "),
            asigna.join("\n")
        )
    };
    let sink = if m.patterns.outbox {
        "this.outbox.stage"
    } else {
        "this.bus.publish"
    };

    let mut cls = vec![
        format!("export abstract class {}Service {{", pascal(svc)),
        ctor.clone(),
        "  static readonly wellKnown = \"/.well-known/axon.json\";".to_string(),
    ];
    for ev in m.emits.keys() {
        // Con outbox, la transaccion es un parametro OBLIGATORIO: es lo que
        // hace imposible escribir el evento fuera de la transaccion que cambia
        // el estado. Sin outbox no hay transaccion que compartir.
        let (firma, paso) = if m.patterns.outbox {
            (
                "data: {}, tx: unknown, cause?: Envelope<unknown>",
                ", tx",
            )
        } else {
            ("data: {}, cause?: Envelope<unknown>", "")
        };
        cls.push(format!(
            "  protected {}({}) {{",
            camel(&format!("emit.{ev}")),
            firma.replace("{}", &pascal(ev))
        ));
        cls.push(format!(
            "    return {sink}(newEnvelope(\"{ev}\", \"{svc}\", data, cause){paso});"
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
    if !m.aggregate.is_empty() {
        out.push(agregados_ts(m));
    }
    if !m.view.is_empty() {
        out.push(vistas_ts(m));
    }
    let routes: Vec<String> = m
        .methods
        .values()
        .filter_map(|me| me.http.clone())
        .map(|h| format!("\"{h}\""))
        .collect();
    if !routes.is_empty() {
        // A declared route nobody serves returns 404 in production and shows up
        // in no test. Startup can refuse — but only if it knows which routes
        // there were supposed to be.
        out.push(format!(
            "\n/** HTTP routes the manifest declares. Startup must fail if any of them\n \
             *  has no handler: a 404 in production tells nobody. */\n\
             export const httpRoutes = [{}] as const;\n",
            routes.join(", ")
        ));
    }
    out.push(format!(
        "\n/** The CAP side declared in the manifest: {c}/{p}.\n \
         *  The isolation level follows from it: paying twice costs more than\n \
         *  retrying, and serving stale data costs less than serving nothing. */\n\
         export const isolationLevel = \"{a}\" as const;\n{st}",
        c = m.cap.consistency,
        p = m.cap.on_partition,
        a = m.cap.aislamiento(),
        st = m
            .cap
            .max_staleness_ms
            .map(|v| format!(
                "/** Staleness budget: data older than this does not get served. */\n\
                 export const maxStalenessMs = {v};\n"
            ))
            .unwrap_or_default(),
    ));
    out.push(flags_ts(m));
    out.push(clients_ts(m, all)?);
    if !m.pii.is_empty() {
        // [A09] Personal data leaks through a log, not through an exploit. The
        // generator hands over the list and the function; using it is up to the
        // person, but they cannot claim not to have known which fields they are.
        out.push(format!(
            "\n/** Fields declared as PII in the manifest. */\nexport const piiFields = [{}] as const;\n\n\
             /** Replaces every PII field with \"[redacted]\", at any depth.\n \
             *  Run anything through here before it reaches a log.\n \
             *\n \
             *  The comparison normalizes: `customer_email` declared in the manifest\n \
             *  covers `customerEmail` in a contract and `customer-email` in a header.\n \
             *  The same concept gets declared once. */\n\
             const normalizePii = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, \"\");\n\
             const pii = new Set((piiFields as readonly string[]).map(normalizePii));\n\n\
             export function redact<T>(value: T): T {{\n  \
               if (Array.isArray(value)) return value.map(redact) as T;\n  \
               if (value === null || typeof value !== \"object\") return value;\n  \
               const out: Record<string, unknown> = {{}};\n  \
               for (const [k, v] of Object.entries(value)) {{\n    \
                 out[k] = pii.has(normalizePii(k)) ? \"[redacted]\" : redact(v);\n  \
               }}\n  return out as T;\n}}\n",
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

/// El tipo TypeScript de la salida de un paso. Es el mismo nombre que ya emite
/// el cliente de la dependencia, asi que el contexto de la saga queda tipado
/// sin inventar tipos nuevos.
fn salida_de(m: &Manifest, r: &str) -> String {
    match Paso::partes(r) {
        // un paso sobre el propio servicio usa sus tipos, sin prefijo
        Some((s, met)) if s == m.service => format!("{}Out", pascal(met)),
        Some((s, met)) => format!("{}{}Out", pascal(s), pascal(met)),
        None => "unknown".to_string(),
    }
}


/// El agregado: la tienda de eventos y el `fold`.
///
/// Lo mecanico se genera —el append con version optimista, la rehidratacion, el
/// switch con un caso por evento declarado— y lo que decide el negocio no: como
/// cada evento cambia el estado lo escribe quien lo sabe, en una interfaz. Un
/// evento declarado sin su caso no compila, que es la unica forma de que
/// agregar un evento no deje el `fold` viejo funcionando en silencio.
pub fn agregados_ts(m: &Manifest) -> String {
    let mut o = vec![
        "\n/** El flujo de un agregado. Es la fuente de verdad: lo que hoy se\n \
         *  guarda en una fila es una PROYECCION de esto.\n \
         *\n \
         *  `append` recibe la version que el llamador creia vigente. Si otro\n \
         *  escribio en medio, tiene que fallar: eso es el UNIQUE (stream_id,\n \
         *  version) haciendo su trabajo, y `axon verify` comprueba que exista\n \
         *  porque sin el las dos escrituras entran y nadie ve un error. */\n\
         /** Un evento tal como esta en el flujo. `en` es CUANDO OCURRIO, y viaja\n \
 *  porque al reconstruir una vista hay que volver a ponerlo: rellenarlo con\n \
 *  la hora de la reconstruccion reescribe el historial en silencio. */\n\
         export interface EventoDelFlujo {\n  \
           version: number;\n  \
           type: string;\n  \
           data: unknown;\n  \
           en: string;\n\
         }\n\
         \n\
         export interface FlujoEventos {\n  \
           /** Los eventos de un flujo, en orden. `desde` permite arrancar de una foto. */\n  \
           leer(streamId: string, desde?: number): Promise<EventoDelFlujo[]>;\n  \
           /** Agrega al final. Rechaza si `esperada` ya no es la ultima version. */\n  \
           append(streamId: string, esperada: number, e: Envelope<unknown>): Promise<number>;\n  \
           /** Las fotos van en `FlujoConFotos`, que solo se genera si el\n   \
            *  manifiesto las declara: opcionales aqui, declararlas y no\n   \
            *  implementarlas compilaria y no haria nada. */\n\
         }\n\
         \n\
         /** Otro escribio primero. No es un error de programa: es la condicion\n \
         *  normal de dos usuarios sobre el mismo agregado, y quien la recibe\n \
         *  vuelve a leer y reintenta. */\n\
         export class VersionEnConflicto extends Error {\n  \
           readonly streamId: string;\n  \
           readonly esperada: number;\n  \
           constructor(streamId: string, esperada: number) {\n    \
             super(`${streamId}: la version ${esperada} ya no es la ultima`);\n    \
             this.streamId = streamId;\n    \
             this.esperada = esperada;\n  \
           }\n\
         }\n"
            .to_string(),
    ];
    if m.aggregate.values().any(|a| a.snapshot_every > 0) {
        o.push(
            "/** Un flujo que ademas guarda fotos.\n \
             *\n \
             *  Una foto es una CACHE del `fold`, y por eso lleva la version de las\n \
             *  reglas con la que se calculo: si el `fold` cambia, las fotos viejas\n \
             *  codifican la version anterior y rehidratar de ahi da un estado que\n \
             *  ya no coincide con reproducir el flujo. Eso no da ningun error: da\n \
             *  un numero equivocado.\n \
             *\n \
             *  `foto` devuelve solo las de la version vigente. Las de otra version\n \
             *  se ignoran y el estado se reconstruye desde el principio, que es\n \
             *  lento y correcto —en ese orden. */\n\
             export interface FlujoConFotos extends FlujoEventos {\n  \
               foto(streamId: string, reglas: number): Promise<{ version: number; estado: unknown } | null>;\n  \
               guardarFoto(streamId: string, version: number, reglas: number, estado: unknown): Promise<void>;\n  \
               /** Borra las fotos que la version vigente no usa: las de OTRA version\n   \
                *  de reglas, y todas menos la mas nueva de cada flujo. Devuelve\n   \
                *  cuantas borro.\n   \
                *\n   \
                *  Puede ser agresiva justamente porque una foto es una CACHE: lo\n   \
                *  peor que puede pasar es reconstruir desde el flujo, que es lento y\n   \
                *  correcto. Borrar de mas no rompe nada; no borrar nunca hace crecer\n   \
                *  la tabla con cada version de reglas.\n   \
                *\n   \
                *  Y no hay carrera con quien rehidrata: `foto` devuelve el estado por\n   \
                *  valor, asi que borrar la fila despues no le quita nada. */\n  \
               limpiarFotos(reglas: number): Promise<number>;\n\
             }\n"
                .to_string(),
        );
    }
    for (nombre, ag) in &m.aggregate {
        let p = pascal(nombre);
        let c = camel(nombre);
        let mut casos = Vec::new();
        let mut aplica = Vec::new();
        for ev in &ag.events {
            let t = pascal(ev);
            casos.push(format!("  aplicar{t}(estado: {p}Estado, e: {t}): {p}Estado;"));
            aplica.push(format!(
                "      case \"{ev}\":\n        \
                   return reglas.aplicar{t}(estado, ev.data as {t});"
            ));
        }
        o.push(format!(
            "/** Los eventos que componen `{nombre}`, declarados en el manifiesto. */\n\
             export const {c}Eventos = [{}] as const;\n\
             export type {p}Evento = typeof {c}Eventos[number];\n",
            ag.events
                .iter()
                .map(|e| format!("\"{e}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        // El estado lo define quien lo sabe: el generador no puede inventar la
        // forma del dominio, solo imponer que haya un caso por evento.
        o.push(format!(
            "/** El estado reconstruido. Su forma la define el dominio; lo que\n \
             *  impone el generador es que haya un caso por cada evento declarado. */\n\
             export interface {p}Reglas<{p}Estado> {{\n  \
               /** El estado antes del primer evento. */\n  \
               inicial(streamId: string): {p}Estado;\n\
             {}\n}}\n",
            casos.join("\n")
        ));
        let gobierna = match &ag.machine {
            Some(mac) => format!(
                "\n *  Gobernado por `[machine.{mac}]`: un evento que no corresponde a\n \
                 *  una transicion legal desde el estado actual se RECHAZA en vez de\n \
                 *  aplicarse. Un flujo con un evento imposible dentro no se puede\n \
                 *  arreglar despues: los eventos no se editan."
            ),
            None => String::new(),
        };
        o.push(format!(
            "/** Reconstruye el estado aplicando el flujo en orden.{gobierna} */\n\
             export function {c}Fold<E>(\n  \
               reglas: {p}Reglas<E>,\n  \
               streamId: string,\n  \
               eventos: {{ version: number; type: string; data: unknown }}[],\n  \
               desde?: {{ version: number; estado: E }},\n\
             ): {{ version: number; estado: E }} {{\n  \
               let estado = desde ? desde.estado : reglas.inicial(streamId);\n  \
               let version = desde ? desde.version : 0;\n  \
               for (const ev of eventos) {{\n    \
                 // El orden no se asume: un hueco en las versiones significa que\n    \
                 // falta un evento, y reconstruir sin el da un estado que nunca\n    \
                 // existio.\n    \
                 if (ev.version !== version + 1) {{\n      \
                   throw new Error(`{nombre}/${{streamId}}: se esperaba la version ${{version + 1}} y llego ${{ev.version}}`);\n    \
                 }}\n    \
                 estado = {c}Aplicar(reglas, estado, ev);\n    \
                 version = ev.version;\n  \
               }}\n  \
               return {{ version, estado }};\n\
             }}\n\
             \n\
             function {c}Aplicar<E>(reglas: {p}Reglas<E>, estado: E, ev: {{ type: string; data: unknown }}): E {{\n  \
               switch (ev.type) {{\n\
             {aplica}\n    \
                 default:\n      \
                   // Un evento en el flujo que el manifiesto no declara: el estado\n      \
                   // que saldria de ignorarlo es incorrecto y nadie lo sabria.\n      \
                   throw new Error(`{nombre}: `+ev.type+` no es un evento declarado del agregado`);\n  \
               }}\n\
             }}\n",
            aplica = aplica.join("\n"),
        ));
        if ag.snapshot_every > 0 {
            o.push(format!(
                "/** Cada cuantos eventos se fotografia, y con que version de reglas.\n \
                 *  Los dos numeros salen del manifiesto: nadie los teclea dos veces. */\n\
                 export const {c}FotoCada = {cada};\n\
                 export const {c}FotoReglas = {reglas};\n\
                 \n\
                 /** Carga el estado: de la ultima foto valida, y solo el resto del\n \
                 *  flujo desde ahi.\n \
                 *\n \
                 *  Si no hay foto de la version vigente, reconstruye entero. Eso es\n \
                 *  lento y correcto, en ese orden: una foto de otra version daria un\n \
                 *  estado incorrecto sin decirlo. */\n\
                 export async function {c}Cargar<E>(\n  \
                   reglas: {p}Reglas<E>,\n  \
                   flujo: FlujoConFotos,\n  \
                   streamId: string,\n\
                 ): Promise<{{ version: number; estado: E }}> {{\n  \
                   const f = await flujo.foto(streamId, {c}FotoReglas);\n  \
                   const desde = f ? {{ version: f.version, estado: f.estado as E }} : undefined;\n  \
                   const eventos = await flujo.leer(streamId, f?.version ?? 0);\n  \
                   return {c}Fold(reglas, streamId, eventos, desde);\n\
                 }}\n\
                 \n\
                 /** Fotografia si toca. Devuelve si la guardo, para poder medirlo:\n \
                 *  una cadencia declarada que no se cumple es una foto que nadie\n \
                 *  sabe que falta. */\n\
                 export async function {c}Fotografiar<E>(\n  \
                   flujo: FlujoConFotos,\n  \
                   streamId: string,\n  \
                   version: number,\n  \
                   estado: E,\n\
                 ): Promise<boolean> {{\n  \
                   if (version === 0 || version % {c}FotoCada !== 0) return false;\n  \
                   await flujo.guardarFoto(streamId, version, {c}FotoReglas, estado);\n  \
                   return true;\n\
                 }}\n",
                cada = ag.snapshot_every,
                reglas = ag.snapshot_version,
            ));
            o.push(format!(
                "/** La ruta que golpea el programador para limpiar las fotos viejas.\n \
                 *  `axon infra` la despliega en los cuatro targets. */\n\
                 export const rutaLimpieza{p} = \"POST /internal/aggregate/{nombre}/limpiar\" as const;\n\
                 \n\
                 /** Una pasada de limpieza. Devuelve cuantas fotos borro, para poder\n \
                 *  medirlo: una limpieza que no reporta nada es indistinguible de una\n \
                 *  que no corre, y lo que se nota entonces es el tamano de la tabla. */\n\
                 export async function limpiar{p}(flujo: FlujoConFotos): Promise<number> {{\n  \
                   return flujo.limpiarFotos({c}FotoReglas);\n\
                 }}\n"
            ));
        }
    }
    o.join("\n")
}

/// La proyeccion: un caso por evento declarado, y el punto donde quedo.
pub fn vistas_ts(m: &Manifest) -> String {
    let mut o = vec![
        "\n/** Donde una vista anota hasta donde llego.\n \
         *\n \
         *  Sin esto, un reinicio reprocesa desde el principio o se salta lo que no\n \
         *  alcanzo a aplicar. Las dos cosas dan una vista incorrecta, y ninguna da\n \
         *  un error: por eso `axon verify` exige la tabla. */\n\
         export interface Checkpoint {\n  \
           /** Desde donde retomar. La ESCRITURA no esta aqui a proposito: la\n   \
            *  posicion tiene que guardarse en la misma transaccion que el efecto\n   \
            *  de la vista, y esa transaccion es de la proyeccion, no del\n   \
            *  framework. En dos transacciones, un corte entre ellas deja la vista\n   \
            *  adelantada o atrasada respecto de lo que dice haber aplicado, y\n   \
            *  ninguna de las dos da un error.\n   \
            *\n   \
            *  Por eso cada `aplicar*` recibe la posicion: la guarda quien puede\n   \
            *  hacerlo junto con el resto.\n   \
            *\n   \
            *  Y es POR FLUJO. La version de un evento es su posicion dentro de su\n   \
            *  flujo, asi que un solo numero para toda la vista no identifica nada\n   \
            *  en cuanto hay mas de un flujo: con uno parecia funcionar. */\n  \
           leer(vista: string, streamId: string): Promise<number>;\n\
         }\n"
            .to_string(),
    ];
    // Reconstruir una vista solo es posible si sus eventos estan LOCALMENTE. Los
    // que llegaron por el bus ya no estan: se consumieron. Asi que la funcion se
    // genera solo cuando todos los eventos de la vista son de un agregado
    // propio, y su ausencia es la respuesta a "se puede reconstruir".
    let reconstruibles: Vec<&String> = m
        .view
        .iter()
        .filter(|(_, vi)| {
            !vi.on.is_empty()
                && vi
                    .on
                    .iter()
                    .all(|ev| m.aggregate.values().any(|a| a.events.contains(ev)))
        })
        .map(|(n, _)| n)
        .collect();
    if !reconstruibles.is_empty() {
        o.push(
            "/** Una vista sombra: se construye aparte y se cambia por la viva de\n \
             *  golpe.\n \
             *\n \
             *  Reconstruir en el sitio deja la vista incompleta mientras corre, y se\n \
             *  sigue leyendo: los que preguntan reciben menos filas de las que hay,\n \
             *  sin error. Con sombra, nadie ve un estado intermedio.\n \
             *\n \
             *  La proyeccion que se le pasa a `reconstruir` tiene que estar apuntada a\n \
             *  la SOMBRA. Si apuntara a la viva, esto seria una reconstruccion en el\n \
             *  sitio con pasos extra —y por eso el demo mide que las lecturas nunca\n \
             *  bajen mientras corre. */\n\
             export interface Sombra {\n  \
               /** Deja la sombra vacia, con su punto en cero. */\n  \
               preparar(): Promise<void>;\n  \
               /** Cambia la sombra por la viva, y su punto con ella, en UNA\n   \
                *  transaccion. En dos, un corte entre ellas deja la vista nueva con el\n   \
                *  punto de la vieja: se saltaria eventos o los reprocesaria. */\n  \
               intercambiar(): Promise<void>;\n\
             }\n\
             \n\
             /** Los flujos del agregado, para recorrerlos. */\n\
             export interface FuenteDeFlujos {\n  \
               flujos(): Promise<string[]>;\n\
             }\n"
                .to_string(),
        );
    }
    for (nombre, vi) in &m.view {
        let p = pascal(nombre);
        let c = camel(nombre);
        let mut casos = Vec::new();
        let mut aplica = Vec::new();
        for ev in &vi.on {
            let t = pascal(ev);
            casos.push(format!(
                "  /** {ev} · guarda `posicion` en la MISMA transaccion que el efecto */\n  \
                 aplicar{t}(e: Envelope<{t}>, posicion: number): Promise<void>;"
            ));
            aplica.push(format!(
                "      case \"{ev}\":\n        \
                   return proyeccion.aplicar{t}(e as Envelope<{t}>, posicion);"
            ));
        }
        o.push(format!(
            "/** La vista `{nombre}`, en `{tabla}`. Un metodo por evento declarado:\n \
             *  agregar uno al manifiesto rompe la compilacion en vez de dejar la\n \
             *  proyeccion vieja corriendo sin enterarse. */\n\
             export interface {p}Proyeccion {{\n{}\n}}\n\
             \n\
             export const {c}Tabla = \"{tabla}\" as const;\n\
             export const {c}Eventos = [{eventos}] as const;\n{atraso}",
            casos.join("\n"),
            tabla = vi.tabla(nombre),
            eventos = vi
                .on
                .iter()
                .map(|e| format!("\"{e}\""))
                .collect::<Vec<_>>()
                .join(", "),
            atraso = match vi.max_staleness_ms {
                Some(ms) => format!(
                    "/** Presupuesto de atraso declarado. Mas viejo que esto no se sirve. */\n\
                     export const {c}AtrasoMaximoMs = {ms};\n"
                ),
                None => String::new(),
            }
        ));
        o.push(format!(
            "/** Rutea el evento al metodo de la vista. El `default` no es\n \
             *  defensivo: un evento que la vista no declara llegaria de una\n \
             *  suscripcion que nadie pidio. */\n\
             export async function {c}Aplicar(\n  \
               proyeccion: {p}Proyeccion,\n  \
               e: Envelope<unknown>,\n  \
               posicion: number,\n\
             ): Promise<void> {{\n  \
               switch (e.type) {{\n\
             {aplica}\n    \
                 default:\n      \
                   throw new Error(`{nombre}: `+e.type+` no es un evento declarado de la vista`);\n  \
               }}\n\
             }}\n\
             \n\
             /** Cuanto atraso lleva la vista, para poder medirlo contra lo declarado. */\n\
             export function {c}Atraso(ultimoEvento: Date, ahora = new Date()): number {{\n  \
               return ahora.getTime() - ultimoEvento.getTime();\n\
             }}\n",
            aplica = aplica.join("\n"),
        ));
        if reconstruibles.contains(&nombre) {
            o.push(format!(
                "/** La ruta que reconstruye la vista. NO lleva cron: reconstruir no es\n \
                 *  periodico, es una operacion que alguien decide. */\n\
                 export const rutaReconstruir{p} = \"POST /internal/view/{nombre}/reconstruir\" as const;\n\
                 \n\
                 /** Tira la vista y la vuelve a construir del flujo. Devuelve cuantos\n \
                 *  eventos aplico.\n \
                 *\n \
                 *  Es lo que convierte un modelo de lectura en algo cuya FORMA se puede\n \
                 *  cambiar sin migracion: se cambia la proyeccion, se reconstruye, y no\n \
                 *  hay `ALTER TABLE` que preserve datos que se pueden recalcular.\n \
                 *\n \
                 *  Se construye en una SOMBRA y se cambia de golpe al final, asi que\n \
                 *  nadie lee un estado intermedio: mientras corre, la vista viva sigue\n \
                 *  respondiendo lo de antes. El intercambio toma un bloqueo breve.\n \
                 *\n \
                 *  El recorrido es por flujo y en orden de version. Una proyeccion cuyo\n \
                 *  resultado dependa del orden ENTRE flujos necesita un orden total que\n \
                 *  el flujo no tiene: ahi esto da un resultado distinto al de la\n \
                 *  proyeccion en vivo, y la comprobacion del demo lo veria. */\n\
                 export async function reconstruir{p}(\n  \
                   sombra: {p}Proyeccion & Sombra,\n  \
                   flujo: FlujoEventos & FuenteDeFlujos,\n\
                 ): Promise<number> {{\n  \
                   await sombra.preparar();\n  \
                   let aplicados = 0;\n  \
                   for (const streamId of await flujo.flujos()) {{\n    \
                     for (const ev of await flujo.leer(streamId)) {{\n      \
                       // Los eventos del flujo no son envelopes: se arma el minimo que\n      \
                       // la proyeccion necesita. El `time` es el del FLUJO, no el de\n      \
                       // ahora: rellenarlo reescribiria el historial en silencio.\n      \
                       if (!({c}Eventos as readonly string[]).includes(ev.type)) continue;\n      \
                       await {c}Aplicar(sombra, {{\n        \
                         id: `${{streamId}}:${{ev.version}}`,\n        \
                         type: ev.type,\n        \
                         source: \"reconstruccion\",\n        \
                         time: ev.en,\n        \
                         traceparent: \"\",\n        \
                         correlationId: streamId,\n        \
                         causationId: null,\n        \
                         data: ev.data,\n      \
                       }}, ev.version);\n      \
                       aplicados++;\n    \
                     }}\n  \
                   }}\n  \
                   // El cambio va al final: hasta aqui nadie vio nada de esto.\n  \
                   await sombra.intercambiar();\n  \
                   return aplicados;\n\
                 }}\n"
            ));
        }
    }
    o.join("\n")
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

/// The coordinator for each saga.
///
/// What gets generated is the PATH: the order of the steps, the journal, the
/// compensation in reverse order and the time budget. What does not get
/// generated is the input to each call, because that is business data — that is
/// the interface someone implements. So a step left unimplemented does not
/// compile, and an implemented step cannot run out of order.
pub fn sagas_ts(m: &Manifest) -> String {
    let mut o = vec![
        "\n/** A saga's journal: where its progress lives. Without it, a restart\n \
         *  halfway through leaves the steps already taken applied and with no\n \
         *  record of which ones: the saga can neither finish nor compensate.\n \
         *\n \
         *  `attempting` is written BEFORE the call and `done` AFTER. A step left\n \
         *  at `attempting` may or may not have happened, so on resume it gets\n \
         *  COMPENSATED, not retried — which is why every compensation has to\n \
         *  tolerate there being nothing to undo.\n \
         *\n \
         *  It also stores the envelope that started the saga. Resuming without it\n \
         *  is impossible: the actions need the call's data, and the process that\n \
         *  held it in memory is precisely the one that died. */\n\
         export interface SagaJournal {\n  \
           open(id: string, saga: string, e: Envelope<unknown>): Promise<void>;\n  \
           mark(id: string, step: number, status: \"attempting\" | \"done\" | \"undone\", output?: unknown): Promise<void>;\n  \
           close(id: string, status: SagaStatus): Promise<void>;\n  \
           /** How far it got, and what each step returned. `null` if it is new.\n   \
            *\n   \
            *  The outputs are needed to COMPENSATE: undoing a step usually needs\n   \
            *  the id THAT step returned, and after a restart that value is in no\n   \
            *  variable — the process that held it is the one that died. Storing it\n   \
            *  in the journal is what keeps compensation possible. */\n  \
           read(id: string): Promise<{ step: number; status: string; outputs: Record<number, unknown> } | null>;\n  \
           /** Claims up to `limit` open sagas that have not moved since\n   \
            *  `olderThan`, and returns each one's envelope.\n   \
            *\n   \
            *  It CLAIMS, it does not list: two instances of the service sweep at\n   \
            *  the same time, and two coordinators on the same saga compensate the\n   \
            *  same steps twice. In Postgres the claim and the filter are the same\n   \
            *  statement:\n   \
            *\n   \
            *    UPDATE saga_<name> SET updated_at = now()\n   \
            *     WHERE status IN ('attempting','done') AND updated_at < $1\n   \
            *     RETURNING id, data\n   \
            *    LIMIT $2\n   \
            *\n   \
            *  Touching `updated_at` IS the claim: the other sweeper no longer sees\n   \
            *  it. And if this process dies halfway, the saga becomes eligible again\n   \
            *  in the next window with nobody unlocking it by hand. */\n  \
           claim(saga: string, olderThan: Date, limit: number): Promise<{ id: string; data: Envelope<unknown> }[]>;\n\
         }\n\
         \n\
         export type SagaStatus = \"completed\" | \"compensated\" | \"stuck\";\n\
         \n\
         /** A compensation that fails has nothing behind it: the saga is left\n \
         *  half-done and needs a person. It is thrown so that does not go\n \
         *  unnoticed. */\n\
         export class SagaStuck extends Error {\n  \
           readonly saga: string;\n  \
           readonly step: number;\n  \
           readonly reason: unknown;\n  \
           constructor(saga: string, step: number, reason: unknown) {\n    \
             super(`${saga}: compensating step ${step} failed; the saga is half-done`);\n    \
             this.saga = saga;\n    \
             this.step = step;\n    \
             this.reason = reason;\n  \
           }\n\
         }\n\
         \n\
         /** What one sweep pass did. It is returned so it can be measured: a\n \
         *  sweep that reports nothing is indistinguishable from one that does not\n \
         *  run. */\n\
         export interface SweepReport {\n  \
           claimed: number;\n  \
           completed: number;\n  \
           compensated: number;\n  \
           /** These need a person. The sweep does NOT retry them. */\n  \
           stuck: number;\n  \
           /** Left for the next pass because the limit was reached. */\n  \
           pending: boolean;\n\
         }\n"
            .to_string(),
    ];
    for (name, sg) in &m.saga {
        let p = pascal(name);
        let c = camel(name);
        let mut actions = Vec::new();
        let mut outputs = Vec::new();
        let mut table = Vec::new();
        for (i, step) in sg.steps.iter().enumerate() {
            let n = i + 1;
            let met = Paso::partes(&step.hacer).map(|(_, x)| x).unwrap_or("?");
            outputs.push(format!("  step{n}?: {};", salida_de(m, &step.hacer)));
            actions.push(format!(
                "  /** step {n} · {} */\n  step{n}{}(e: Envelope<unknown>, prior: {p}Outputs): Promise<{s}>;",
                step.hacer,
                pascal(met),
                p = p,
                s = salida_de(m, &step.hacer)
            ));
            match &step.undo {
                Some(u) => {
                    let umet = Paso::partes(u).map(|(_, x)| x).unwrap_or("?");
                    actions.push(format!(
                        "  /** undoes step {n} · {u} · receives what the earlier steps returned,\n   \
                         *  and has to tolerate there being nothing to undo */\n  \
                         undo{n}{}(e: Envelope<unknown>, prior: {p}Outputs): Promise<void>;",
                        pascal(umet),
                        p = p
                    ));
                    table.push(format!(
                        "  {{ step: {n}, run: \"{}\", undo: \"{u}\" }},",
                        step.hacer
                    ));
                }
                None => table.push(format!(
                    "  // the last step carries no compensation: if it fails, there is\n  \
                     // nothing of its own to undo\n  \
                     {{ step: {n}, run: \"{}\", undo: null }},",
                    step.hacer
                )),
            }
        }
        o.push(format!(
            "/** The route the scheduler hits to run one sweep pass. `axon infra`\n \
             *  deploys it on all four targets, so startup has to serve it by calling\n \
             *  `sweep{p}`: a scheduler pointed at a 404 applies without an error and\n \
             *  sweeps nothing.\n \
             *\n \
             *  It is NOT a declared method, so it does not go out through the\n \
             *  gateway. It triggers compensations: it cannot be public. */\n\
             export const sweepRoute{p} = \"POST /internal/saga/{name}/sweep\" as const;\n"
        ));
        o.push(format!(
            "/** The steps declared in the manifest. Generated: do not edit. */\n\
             export const {c}Steps = [\n{}\n] as const;\n",
            table.join("\n")
        ));
        o.push(format!(
            "/** What each step returned, stored in the journal. It is what makes\n \
             *  compensating possible after a restart: undoing a step usually needs\n \
             *  the id that step returned, and a variable in memory does not survive\n \
             *  the process that held it. */\n\
             export interface {p}Outputs {{\n{}\n}}\n",
            outputs.join("\n")
        ));
        o.push(format!(
            "/** One method per step and one per compensation. They are implemented\n \
             *  by whoever knows the data: the coordinator knows the order, not the\n \
             *  contents. */\n\
             export interface {p}Actions {{\n{}\n}}\n",
            actions.join("\n")
        ));

        // the body: forward until something fails, and back again
        let mut forward = Vec::new();
        let mut backward = Vec::new();
        let mut rehydrate = Vec::new();
        for (i, step) in sg.steps.iter().enumerate() {
            let n = i + 1;
            let met = Paso::partes(&step.hacer).map(|(_, x)| x).unwrap_or("?");
            rehydrate.push(format!(
                "    step{n}: saved[{n}] as {} | undefined,",
                salida_de(m, &step.hacer)
            ));
            forward.push(format!(
                "      case {n}:\n        \
                   await journal.mark(id, {n}, \"attempting\");\n        \
                   prior.step{n} = await actions.step{n}{}(e, prior);\n        \
                   // the output is stored WITH the `done`: in two writes, a crash\n        \
                   // between them leaves the step done and its result lost\n        \
                   await journal.mark(id, {n}, \"done\", prior.step{n});\n        \
                   break;",
                pascal(met)
            ));
            if let Some(u) = &step.undo {
                let umet = Paso::partes(u).map(|(_, x)| x).unwrap_or("?");
                backward.push(format!(
                    "      case {n}:\n        \
                       await actions.undo{n}{}(e, prior);\n        \
                       break;",
                    pascal(umet)
                ));
            } else {
                backward.push(format!(
                    "      case {n}:\n        \
                       break; // no compensation declared: this is the last step"
                ));
            }
        }
        let budget = match sg.timeout_ms {
            Some(ms) => format!("{ms}"),
            None => "Number.POSITIVE_INFINITY".to_string(),
        };
        o.push(format!(
            "/** Runs the `{name}` saga.\n \
             *\n \
             *  Forward until a step fails or the budget runs out; from there in\n \
             *  REVERSE order, undoing only what was attempted. Reverse order is not\n \
             *  aesthetics: compensating forward undoes a step whose effect a later\n \
             *  step already used.\n \
             *\n \
             *  If `id` already has a journal, it resumes: the step left at\n \
             *  `attempting` gets compensated, because there is no telling whether it\n \
             *  happened. */\n\
             export async function run{p}(\n  \
               id: string,\n  \
               actions: {p}Actions,\n  \
               journal: SagaJournal,\n  \
               e: Envelope<unknown>,\n\
             ): Promise<{{ status: SagaStatus; upTo: number; error?: unknown }}> {{\n  \
               const total = {total};\n  \
               // The budget declared in the manifest; `axon verify` already checked\n  \
               // that it covers the sum of the steps and their compensations.\n  \
               const deadline = Date.now() + {budget};\n  \
               const previous = await journal.read(id);\n  \
               if (!previous) await journal.open(id, \"{name}\", e);\n  \
               // Rehydrated from the journal, not from a variable: on resume this is\n  \
               // all that is left of what the earlier steps did.\n  \
               //\n  \
               // The journal stores them by step NUMBER and here they are named: a\n  \
               // cast from one shape to the other compiles and leaves everything\n  \
               // `undefined`, so the translation is explicit, field by field.\n  \
               const saved = previous?.outputs ?? {{}};\n  \
               const prior: {p}Outputs = {{\n{rehydrate}\n  \
               }};\n  \
               // a half-attempted step is not retried: it is undone\n  \
               let done = previous ? (previous.status === \"done\" ? previous.step : previous.step - 1) : 0;\n  \
               const doubtful = previous?.status === \"attempting\" ? previous.step : 0;\n  \
               // The step that FAILED gets undone too: a timeout does not say that\n  \
               // nothing happened on the other side. Compensating only up to the last\n  \
               // success leaves that effect applied forever.\n  \
               let attempted = doubtful;\n  \
               let failure: unknown = doubtful ? new Error(\"resumed with a step in doubt\") : undefined;\n  \
               if (!doubtful) {{\n    \
                 for (let step = done + 1; step <= total; step++) {{\n      \
                   if (Date.now() > deadline) {{\n        \
                     failure = new Error(`{name}: budget exhausted before step ${{step}}`);\n        \
                     break;\n      \
                   }}\n      \
                   attempted = step;\n      \
                   try {{\n        \
                     await runStep{p}(step, actions, journal, e, id, prior);\n        \
                     done = step;\n      \
                   }} catch (err) {{\n        \
                     failure = err;\n        \
                     break;\n      \
                   }}\n    \
                 }}\n  \
               }}\n  \
               if (!failure) {{\n    \
                 await journal.close(id, \"completed\");\n    \
                 return {{ status: \"completed\", upTo: total }};\n  \
               }}\n  \
               // back again: everything that was ATTEMPTED, in reverse order\n  \
               for (let step = attempted; step >= 1; step--) {{\n    \
                 try {{\n      \
                   await undoStep{p}(step, actions, e, prior);\n      \
                   await journal.mark(id, step, \"undone\");\n    \
                 }} catch (err) {{\n      \
                   await journal.close(id, \"stuck\");\n      \
                   throw new SagaStuck(\"{name}\", step, err);\n    \
                 }}\n  \
               }}\n  \
               await journal.close(id, \"compensated\");\n  \
               return {{ status: \"compensated\", upTo: done, error: failure }};\n\
             }}\n\
             \n\
             async function runStep{p}(\n  \
               step: number,\n  \
               actions: {p}Actions,\n  \
               journal: SagaJournal,\n  \
               e: Envelope<unknown>,\n  \
               id: string,\n  \
               prior: {p}Outputs,\n\
             ): Promise<void> {{\n  \
               switch (step) {{\n\
             {forward}\n    \
                 default:\n      \
                   throw new Error(`{name}: step ${{step}} is not declared in the manifest`);\n  \
               }}\n\
             }}\n\
             \n\
             async function undoStep{p}(step: number, actions: {p}Actions, e: Envelope<unknown>, prior: {p}Outputs): Promise<void> {{\n  \
               switch (step) {{\n\
             {backward}\n    \
                 default:\n      \
                   throw new Error(`{name}: step ${{step}} is not declared in the manifest`);\n  \
               }}\n\
             }}\n",
            total = sg.steps.len(),
            forward = forward.join("\n"),
            backward = backward.join("\n"),
            rehydrate = rehydrate.join("\n"),
        ));

        // The sweep: whoever calls back a saga that got stranded.
        //
        // The coordinator already knows how to resume, but nobody calls it: the
        // process that had it in flight is the one that died. Without this, a
        // saga with a step at `attempting` stays there forever, and the journal
        // records it with nobody reading it.
        let window = sg.timeout_ms.unwrap_or(60_000);
        o.push(format!(
            "/** One sweep pass: resumes the `{name}` sagas that are not moving.\n \
             *\n \
             *  It only touches those idle for longer than their own BUDGET\n \
             *  ({window}ms). That threshold is not a heuristic: `axon verify` already\n \
             *  checked that the budget covers the sum of the steps and their\n \
             *  compensations, so a saga older than that is not on its way — it is\n \
             *  stranded. Sweeping sooner would mean running a second coordinator over\n \
             *  a live saga.\n \
             *\n \
             *  One left `stuck` is NOT retried: it is counted and left alone. A\n \
             *  compensation that already failed needs a person, and retrying it\n \
             *  quietly hides exactly that. */\n\
             export async function sweep{p}(\n  \
               actions: {p}Actions,\n  \
               journal: SagaJournal,\n  \
               limit = 50,\n\
             ): Promise<SweepReport> {{\n  \
               const olderThan = new Date(Date.now() - {window});\n  \
               const stranded = await journal.claim(\"{name}\", olderThan, limit);\n  \
               const r: SweepReport = {{\n    \
                 claimed: stranded.length,\n    \
                 completed: 0,\n    \
                 compensated: 0,\n    \
                 stuck: 0,\n    \
                 // If the limit filled up there are more waiting, and saying so is\n    \
                 // the difference between a sweep that keeps up and one that does not.\n    \
                 pending: stranded.length >= limit,\n  \
               }};\n  \
               for (const {{ id, data }} of stranded) {{\n    \
                 try {{\n      \
                   const out = await run{p}(id, actions, journal, data);\n      \
                   if (out.status === \"completed\") r.completed++;\n      \
                   else r.compensated++;\n    \
                 }} catch (err) {{\n      \
                   // A stuck saga does not abort the pass: the others are still\n      \
                   // stranded and this is the only thing that will look at them.\n      \
                   if (err instanceof SagaStuck) r.stuck++;\n      \
                   else throw err;\n    \
                 }}\n  \
               }}\n  \
               return r;\n\
             }}\n\
             \n\
             /** Starts the periodic sweep and returns how to stop it.\n \
             *\n \
             *  The interval comes from the declared budget: nothing becomes eligible\n \
             *  sooner, so sweeping more often is work without a result. It is safe\n \
             *  with several instances because `claim` claims.\n \
             *\n \
             *  `onPass` receives every pass. Wire it to your metrics: a sweep that\n \
             *  reports nothing is indistinguishable from one that does not run, and\n \
             *  this is the only place from which a stuck saga becomes visible. */\n\
             export function startSweep{p}(\n  \
               actions: {p}Actions,\n  \
               journal: SagaJournal,\n  \
               onPass: (r: SweepReport) => void,\n  \
               intervalMs = {window},\n\
             ): () => void {{\n  \
               let running = false;\n  \
               const t = setInterval(async () => {{\n    \
                 // Without this, a slow pass overlaps the next one in the same\n    \
                 // process and both of them claim.\n    \
                 if (running) return;\n    \
                 running = true;\n    \
                 try {{\n      \
                   onPass(await sweep{p}(actions, journal));\n    \
                 }} finally {{\n      \
                   running = false;\n    \
                 }}\n  \
               }}, intervalMs);\n  \
               // a background sweep should not keep the process alive on shutdown\n  \
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

// ---------- resilient clients ----------

const RESILIENCE_TS: &str = r#"
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
"#;

/// Typed clients for the declared dependencies.
///
/// The numbers are not a suggestion: the timeout, the retries and the breaker
/// come from `[[depends]]` and land literally in the emitted code, so the
/// policy that ships is the policy that was declared.
fn clients_ts(m: &Manifest, all: &[Manifest]) -> Result<String, String> {
    if m.depends.is_empty() {
        return Ok(String::new());
    }
    let mut types = Vec::new();
    let mut methods = Vec::new();
    for d in &m.depends {
        let tgt = d.target();
        let other = all
            .iter()
            .find(|o| o.service == tgt)
            .ok_or_else(|| format!("{}: depends on {tgt}, which has no manifest", m.service))?;
        let sig = other
            .methods
            .get(&d.method)
            .ok_or_else(|| format!("{}: {tgt} does not expose `{}`", m.service, d.method))?;
        // Prefixed with the service: another service's types cannot collide
        // with our own.
        let base = format!("{}{}", pascal(tgt), pascal(&d.method));
        types.push(iface(&format!("{base}In"), &sig.input));
        types.push(iface(&format!("{base}Out"), &sig.output));

        let pol = format!(
            "{{ timeoutMs: {}, retries: {}, breaker: {} }}",
            d.timeout_ms.unwrap_or(10_000),
            d.retries,
            d.breaker
        );
        // `on_partition = "degrade"` makes the degraded path a required
        // argument: the client cannot be called without saying what gets served
        // while the other side is unreachable.
        let (param, body) = if m.cap.degrada() {
            (
                format!(", fallback: () => Promise<{base}Out>"),
                concat!(
                    "    try {\n",
                    "      return await attempt();\n",
                    "    } catch {\n",
                    "      // declared `degrade`: serving something stale beats serving nothing\n",
                    "      return fallback();\n",
                    "    }"
                )
                .to_string(),
            )
        } else {
            (String::new(), "    return attempt();".to_string())
        };
        methods.push(format!(
            "  /** {tgt}.{met} · timeout {t}ms · {r} retries · breaker {b} */\n  \
             async {name}(input: {base}In, e: Envelope<unknown>{param}): Promise<{base}Out> {{\n    \
               const attempt = () => withPolicy(\"{tgt}.{met}\", {pol}, async () =>\n      \
                 (await this.transport.call(\"{tgt}\", \"{met}\", input, headers(e, {idem}))) as {base}Out);\n\
{body}\n  \
             }}",
            met = d.method,
            t = d.timeout_ms.unwrap_or(10_000),
            r = d.retries,
            b = d.breaker,
            name = camel(&format!("{tgt}.{}", d.method)),
            idem = sig.is_idempotent(),
        ));
    }
    Ok(format!(
        "{}\n{}\n\n/** Clients for the dependencies declared in the manifest. */\n\
         export class Clients {{\n  \
           protected readonly transport: Transport;\n  \
           constructor(transport: Transport) {{\n    this.transport = transport;\n  }}\n\
         {}\n}}\n",
        RESILIENCE_TS,
        types.join("\n"),
        methods.join("\n")
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
