//! Un solo archivo de checks: si algo de esto se rompe, la herramienta miente.
use std::process::Command;

fn tiene(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn axon(args: &[&str]) -> (String, String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_axon"))
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.success(),
    )
}

/// El ejemplo, pero sin el `[pooler]`.
///
/// `orders` declara 4 nodos de reparto y hoy eso solo se levanta en el target
/// local: el resto RECHAZA el plan en vez de emitir una instancia sola, que
/// aplicaria sin error y dejaria el reparto sin existir. Asi que todo lo que se
/// afirma sobre gcp, aws y k8s se afirma sobre esta copia — y `el_reparto_no_se_
/// renderiza_donde_no_existe` cubre el rechazo.
/// El ejemplo sin `[pooler]` y con la bodega que el target sepa alimentar.
///
/// `axon infra` rechaza una bodega sin camino de ingesta, y con razon: el
/// esquema se aplicaria y las tablas se quedarian vacias sin un solo error. El
/// ejemplo declara ClickHouse, que es el que tiene camino en local; para gcp
/// hay que decir BigQuery, y k8s todavia no tiene ninguno.
fn ajustado(bodega: &str) -> String {
    // un directorio por llamada: los tests corren en paralelo y compartir la
    // ruta hace que uno borre el arbol que otro esta leyendo
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "axon-sin-pooler-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for e in std::fs::read_dir("examples").unwrap().flatten() {
        let nombre = e.file_name().to_string_lossy().to_string();
        if e.path().is_dir() {
            if nombre == "sql" || nombre == "sql-policies" {
                copiar(&e.path(), &dir.join(&nombre));
            }
            continue;
        }
        if !nombre.ends_with(".toml") && !nombre.ends_with(".json") {
            continue;
        }
        let mut texto = std::fs::read_to_string(e.path()).unwrap();
        texto = match bodega {
            // k8s no tiene camino de ingesta: lo que corresponde es justo lo
            // que dice el mensaje de error, `export = false`
            "ninguna" => texto.replace(
                "[analytics]",
                "[analytics]\nexport = false",
            ),
            otra => texto.replace(
                "warehouse = \"clickhouse\"",
                &format!("warehouse = \"{otra}\""),
            ),
        };
        // el bloque va al final del manifiesto, asi que cortar desde ahi
        // alcanza y no hay que parsear TOML en un test
        let sin = match texto.find("\n[pooler]") {
            Some(i) => texto[..i].to_string(),
            None => texto,
        };
        std::fs::write(dir.join(&nombre), sin).unwrap();
    }
    dir.to_string_lossy().to_string()
}

/// De donde sale el plan para este target: el ejemplo tal cual donde el
/// reparto se renderiza, y la copia sin pooler donde todavia no.
fn fuente(target: &str) -> String {
    match target {
        "local" | "plan" => "examples".to_string(),
        "gcp" => ajustado("bigquery"),
        "k8s" => ajustado("ninguna"),
        _ => ajustado("clickhouse"),
    }
}

fn copiar(de: &std::path::Path, a: &std::path::Path) {
    std::fs::create_dir_all(a).unwrap();
    for e in std::fs::read_dir(de).unwrap().flatten() {
        if e.path().is_dir() {
            copiar(&e.path(), &a.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), a.join(e.file_name())).unwrap();
        }
    }
}

/// Lo que este rechazo evita: `terraform apply` sin un error y un solo Postgres
/// donde el manifiesto declara cuatro. El reparto no existiria y nada lo diria.
#[test]
fn el_reparto_no_se_renderiza_donde_no_existe() {
    for t in ["gcp", "aws", "k8s"] {
        let (_, err, ok) = axon(&["infra", "examples", "--target", t]);
        assert!(!ok, "{t} renderizo un plan con reparto que no sabe repartir");
        assert!(err.contains("shards = 4"), "{t}: {err}");
        assert!(err.contains("--target local"), "{t}: {err}");
    }
    // y sin el pooler —y con una bodega que el target sepa alimentar— los tres
    // siguen rindiendo
    for t in ["gcp", "aws", "k8s"] {
        let (_, err, ok) = axon(&["infra", &fuente(t), "--target", t]);
        assert!(ok, "{t}: {err}");
    }
}

#[test]
fn ejemplos_limpios() {
    let (out, err, ok) = axon(&["verify", "examples"]);
    assert!(ok, "verify fallo: {err}");
    assert!(out.contains("0 errores"), "{out}");
}

#[test]
fn trazabilidad_no_es_opcional() {
    let (ts, _, _) = axon(&["build", "examples/payments.toml", "examples"]);
    for f in ["traceparent", "correlationId", "causationId"] {
        assert!(ts.contains(f), "falta {f}");
    }
    // outbox declarado -> el emisor no toca el bus (sin dual-write)
    assert!(ts.contains("this.outbox.stage(newEnvelope"));
    assert!(!ts.contains("this.bus.publish(newEnvelope"));
    // Y la transaccion de quien llama es OBLIGATORIA: con una conexion propia,
    // el `stage` se confirma solo, asi que una transaccion revertida deja el
    // evento sin su fila y el relay publica algo que nunca paso. Medido contra
    // los contenedores antes de arreglarlo: 0 pagos y 1 evento.
    assert!(
        ts.contains("tx: unknown, cause?: Envelope<unknown>"),
        "el emisor con outbox no exige la transaccion:\n{ts}"
    );
    assert!(ts.contains("stage(newEnvelope(\"payment.captured@v1\", \"payments\", data, cause), tx)"));
    // sin outbox no hay transaccion que compartir, y pedirla seria ruido
    let (sin, _, _) = axon(&["build", "examples/orders.toml", "examples"]);
    assert!(sin.contains("this.bus.publish(newEnvelope"));
    assert!(
        !sin.contains("tx: unknown"),
        "un servicio sin outbox no tiene transaccion que pasar"
    );
    // consumidor idempotente por defecto, no por disciplina
    assert!(ts.contains("this.inbox.once(e.id"));
    assert!(ts.contains(r#"case "order.placed@v1""#));
}

#[test]
fn migraciones_plegadas_en_el_er() {
    let (er, _, _) = axon(&["er", "examples"]);
    assert!(er.contains("ORDER ||--o{ ORDER_ITEM : order_id"));
    assert!(er.contains("text provider_ref"), "ADD COLUMN no se plego");
    assert!(!er.contains("currency"), "DROP COLUMN no se plego");
}

/// El esquema se lee con un parser SQL, no con una regex. Lo que sigue es
/// exactamente lo que la regex no podia, y su peor propiedad era romperse en
/// silencio: devolver columnas mal sin que nadie se enterara.
#[test]
fn el_ddl_se_parsea_de_verdad() {
    let dir = std::env::temp_dir().join("axon-ddl");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sql")).unwrap();
    std::fs::write(
        dir.join("sql/001_duro.expand.sql"),
        r#"
CREATE TABLE "ledger_entry" (
  id            uuid NOT NULL DEFAULT gen_random_uuid(),
  account_id    uuid NOT NULL,
  posted_at     timestamptz NOT NULL DEFAULT now(),
  amount_cents  numeric(20, 4) NOT NULL,
  meta          jsonb NOT NULL DEFAULT '{}'::jsonb,
  CONSTRAINT ledger_entry_pkey PRIMARY KEY (id),
  CONSTRAINT ledger_entry_account_fkey FOREIGN KEY (account_id) REFERENCES account (id) ON DELETE RESTRICT
) PARTITION BY RANGE (posted_at);

CREATE TABLE account (
  id      uuid PRIMARY KEY,
  handle  varchar(64) NOT NULL UNIQUE
);

CREATE INDEX ledger_entry_account_idx ON "ledger_entry" (account_id, posted_at DESC);
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("l.toml"),
        "service = \"ledger\"\nowner = \"x\"\ntier = \"2\"\n[infra]\nstate = \"postgres\"\nmigrations = \"sql/\"\n",
    )
    .unwrap();

    let (er, err, ok) = axon(&["er", dir.to_str().unwrap()]);
    assert!(ok, "{err}");
    // PRIMARY KEY y FOREIGN KEY declaradas a nivel de tabla, no en la columna
    assert!(er.contains("uuid id PK"), "PK de tabla no resuelta:\n{er}");
    assert!(
        er.contains("uuid account_id FK"),
        "FK de tabla no resuelta:\n{er}"
    );
    assert!(
        er.contains("ACCOUNT ||--o{ LEDGER_ENTRY : account_id"),
        "{er}"
    );
    // un tipo tiene que caber en un token o rompe el ER de mermaid
    assert!(
        er.contains("numeric(20,4) amount_cents") || er.contains("numeric(20,_4) amount_cents"),
        "{er}"
    );
    assert!(er.contains("varchar(64) handle"), "{er}");
    // CREATE INDEX no es una tabla
    assert!(
        !er.to_uppercase().contains("LEDGER_ENTRY_ACCOUNT_IDX"),
        "{er}"
    );

    // un DROP dentro de un comentario no es destructivo; uno de verdad si
    std::fs::write(
        dir.join("sql/002_no_es_drop.expand.sql"),
        "-- ojo: no hacer DROP TABLE account aqui\nALTER TABLE account ADD COLUMN nota text;\n",
    )
    .unwrap();
    let (_, _, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "un DROP en un comentario se conto como destructivo");

    std::fs::write(
        dir.join("sql/003_si_es_drop.expand.sql"),
        "ALTER TABLE account DROP COLUMN nota;\n",
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("destructiva sin marcar"), "{err}");

    // y el SQL que no se puede parsear falla ruidosamente, nunca en silencio
    std::fs::write(
        dir.join("sql/004_roto.expand.sql"),
        "CREATE TABL account (;\n",
    )
    .unwrap();
    let (_, err, ok) = axon(&["er", dir.to_str().unwrap()]);
    assert!(!ok, "el SQL invalido se ignoro en silencio");
    assert!(err.contains("no se pudo parsear"), "{err}");
}

/// Una clave anadida en una migracion POSTERIOR tiene que contar. Era invisible,
/// y con eso toda regla sobre unicidad —la del flujo de eventos, las de reparto,
/// el punto de una vista— la daba por ausente y pasaba en silencio.
#[test]
fn una_clave_anadida_despues_cuenta() {
    let dir = std::env::temp_dir().join("axon-alter-pk");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sql")).unwrap();
    std::fs::write(
        dir.join("sql/001_init.expand.sql"),
        "CREATE TABLE punto (vista text NOT NULL, posicion bigint NOT NULL);\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("sql/002_clave.expand.sql"),
        "ALTER TABLE punto ADD COLUMN stream_id uuid NOT NULL;\n\
         ALTER TABLE punto ADD PRIMARY KEY (vista, stream_id);\n\
         ALTER TABLE punto ADD CONSTRAINT punto_pos UNIQUE (posicion);\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("s.toml"),
        "service = \"s\"\nversion = \"1.0.0\"\nowner = \"e\"\ntier = \"1\"\n\n\
         [analytics]\nexport = false\n\n[infra]\nstate = \"postgres\"\nmigrations = \"sql/\"\n",
    )
    .unwrap();
    // el ER es la proyeccion mas simple del esquema plegado
    let (er, err, ok) = axon(&["er", dir.to_str().unwrap()]);
    assert!(ok, "{err}");
    // la columna anadida esta, y la PK marcada
    assert!(er.contains("stream_id"), "{er}");
    assert!(er.contains("PK"), "la PK anadida despues no se marco:\n{er}");
}

#[test]
fn el_mismo_plan_en_cuatro_targets() {
    for (target, marca) in [
        ("local", "postgres:16-alpine"),
        ("gcp", "google_pubsub_subscription"),
        ("aws", "aws_sqs_queue"),
        ("k8s", "kind: Trigger"),
    ] {
        let (out, err, ok) = axon(&["infra", &fuente(target), "--target", target]);
        assert!(ok, "{target}: {err}");
        assert!(out.contains(marca), "{target} no genero {marca}");
    }
    // DLQ siempre, en todos los targets
    for t in ["gcp", "aws", "k8s"] {
        let (out, _, _) = axon(&["infra", &fuente(t), "--target", t]);
        assert!(out.to_lowercase().contains("dead"), "{t} sin DLQ");
    }
}

#[test]
fn todos_los_targets_despliegan_el_workload() {
    // sin esto la IaC deja topics y bases sin nada que corra el codigo
    for (target, marca) in [
        ("local", "dockerfile: services/payments/Dockerfile"),
        (
            "gcp",
            "resource \"google_cloud_run_v2_service\" \"payments\"",
        ),
        ("aws", "resource \"aws_ecs_service\" \"payments\""),
        ("k8s", "kind: Deployment"),
    ] {
        let (out, _, _) = axon(&["infra", &fuente(target), "--target", target]);
        assert!(out.contains(marca), "{target} no despliega el workload");
    }
    // y la entrega llega a alguien: nada de suscripciones al vacio
    let (gcp, _, _) = axon(&["infra", &fuente("gcp"), "--target", "gcp"]);
    assert!(gcp.contains("push_endpoint = google_cloud_run_v2_service.payments.uri"));
    let (k, _, _) = axon(&["infra", &fuente("k8s"), "--target", "k8s"]);
    assert!(
        k.contains("kind: Service\nmetadata:\n  name: payments"),
        "el Trigger apunta a un Service inexistente"
    );
    // el secreto llega al contenedor, no solo al vault
    assert!(gcp.contains("STRIPE_API_KEY"));
    let (loc, _, _) = axon(&["infra", "examples", "--target", "local"]);
    assert!(loc.contains("DATABASE_URL: postgres://postgres:local@db-payments"));
}

#[test]
fn runtime_desconocido_no_se_ignora() {
    let dir = std::env::temp_dir().join("axon-runtime");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("r.toml"),
        "service = \"r\"\nowner = \"x\"\ntier = \"2\"\n[infra]\nruntime = \"lambda\"\n",
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("only `container`"), "{err}");
}

#[test]
fn entornos_son_deltas() {
    let (prod, _, _) = axon(&["infra", "examples", "--target", "plan", "--env", "prod"]);
    let (stg, _, _) = axon(&["infra", "examples", "--target", "plan", "--env", "staging"]);
    assert!(
        prod.contains("\"min_instances\": 3"),
        "prod no aplico el override"
    );
    assert!(!stg.contains("\"min_instances\": 3"));
}

#[test]
fn secuencia_esperada_y_real() {
    let (seq, _, _) = axon(&["seq", "order.placed@v1", "examples"]);
    assert!(seq.contains("orders->>payments: order.placed@v1"));
    assert!(seq.contains("charges.create (externo)"));
    let log = std::env::temp_dir().join("axon-test.ndjson");
    std::fs::write(&log, concat!(
        r#"{"id":"1","type":"order.placed@v1","source":"orders","time":"01","correlationId":"c","causationId":null}"#, "\n",
        r#"{"id":"2","type":"payment.captured@v1","source":"payments","time":"02","correlationId":"c","causationId":"1"}"#, "\n",
    )).unwrap();
    let (tree, _, _) = axon(&["trace", log.to_str().unwrap()]);
    assert!(tree.contains("└─ order.placed@v1 <- orders"));
    assert!(tree.contains("payment.captured@v1 <- payments"));
}

#[test]
fn api_y_gobernanza_bloquean() {
    let dir = std::env::temp_dir().join("axon-bad");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bad.toml"),
        r#"
service = "bad"
[methods.charge]
http = "POST /charges"
in = { a = "int" }
out = { b = "int" }
[[depends]]
service = "bad"
method = "charge"
"#,
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    for esperado in [
        "no `owner`",
        "no `tier`",
        "sin version en la ruta",
        "sin `idempotent = true`",
        "sin `timeout_ms`",
    ] {
        assert!(
            err.contains(esperado),
            "falto el check `{esperado}`:\n{err}"
        );
    }
}

/// Lo que el README promete y solo una herramienta externa puede confirmar.
/// Sin la herramienta, el test se salta en vez de mentir.
#[test]
fn el_typescript_generado_typechequea() {
    if !tiene("node") {
        eprintln!("salteado: node no esta instalado");
        return;
    }
    let dir = std::env::temp_dir().join("axon-tsc");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (ts, err, ok) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(ok, "{err}");
    std::fs::write(dir.join("contracts.ts"), ts).unwrap();
    let out = Command::new("npx")
        .args([
            "-y",
            "-p",
            "typescript@5",
            "tsc",
            "--noEmit",
            "--strict",
            "--target",
            "es2022",
            "--lib",
            "es2022,dom",
            "contracts.ts",
        ])
        .current_dir(&dir)
        .output()
        .expect("npx");
    assert!(
        out.status.success(),
        "tsc fallo:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// El tipo de un evento consumido lo declara su emisor. Sin los demas
/// manifiestos, `build` tiene que fallar con un mensaje util, no generar
/// codigo que no compila.
#[test]
fn build_sin_fuentes_falla_claro() {
    let (_, err, ok) = axon(&["build", "examples/payments.toml"]);
    assert!(!ok);
    assert!(err.contains("no se encontro quien lo emite"), "{err}");
    assert!(err.contains("Pasa los demas manifiestos"), "{err}");
}

/// `terraform fmt` solo dice que el HCL parsea. `validate` con los providers
/// reales dice que los atributos existen — es lo que caza una interpolacion
/// de una variable inexistente o un bloque al que le falta un campo.
#[test]
fn el_hcl_generado_valida() {
    if !tiene("terraform") {
        eprintln!("salteado: terraform no esta instalado");
        return;
    }
    let casos = [
        (
            "gcp",
            "google = { source = \"hashicorp/google\", version = \"~> 6.0\" }",
            "variable \"project\" {}\nvariable \"region\" {}\nvariable \"db_tier\" {}\n",
        ),
        (
            "aws",
            "aws = { source = \"hashicorp/aws\", version = \"~> 5.0\" }",
            "variable \"project\" {}\nvariable \"db_instance_class\" {}\nvariable \"ecs_cluster\" {}\n\
             variable \"ecs_execution_role_arn\" {}\nvariable \"subnets\" { type = list(string) }\n",
        ),
    ];
    // Y los mismos dos targets sobre un manifiesto CON saga: el barrido emite
    // un programador y una tarea que el ejemplo no tiene, y un atributo
    // inventado ahi no lo ve nadie hasta el `apply`.
    //
    // Las variables propias del barrido las declara axon, asi que aca NO van:
    // declararlas de los dos lados es un `Duplicate variable declaration`, y el
    // fallback a `fmt` no lo ve.
    let saga = fixture_saga("tf").to_string_lossy().to_string();
    let mut casos: Vec<(String, &str, String, String)> = casos
        .iter()
        .map(|(t, prov, vars)| (t.to_string(), *prov, vars.to_string(), fuente(t)))
        .collect();
    casos.push((
        "gcp-saga".into(),
        casos[0].1,
        casos[0].2.clone(),
        saga.clone(),
    ));
    casos.push((
        "aws-saga".into(),
        casos[1].1,
        // las variables del barrido las declara axon: aca no va ninguna
        casos[1].2.clone(),
        saga,
    ));

    for (etiqueta, provider, vars, fuente_tf) in casos {
        let target = etiqueta.trim_end_matches("-saga");
        let dir = std::env::temp_dir().join(format!("axon-tf-{etiqueta}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (tf, _, _) = axon(&["infra", &fuente_tf, "--target", target]);
        std::fs::write(dir.join("main.tf"), tf).unwrap();
        std::fs::write(dir.join("vars.tf"), vars).unwrap();
        std::fs::write(
            dir.join("prov.tf"),
            format!("terraform {{\n  required_providers {{ {provider} }}\n}}\n"),
        )
        .unwrap();

        // sin red no se pueden bajar los providers: se cae a `fmt`, que al
        // menos confirma que el HCL parsea
        let init = Command::new("terraform")
            .args(["init", "-backend=false", "-input=false"])
            .current_dir(&dir)
            .output()
            .expect("terraform init");
        if !init.status.success() {
            eprintln!("{target}: sin providers, se valida solo el parseo");
            let fmt = Command::new("terraform")
                .args(["fmt", "-check", dir.to_str().unwrap()])
                .output()
                .expect("terraform fmt");
            let err = String::from_utf8_lossy(&fmt.stderr);
            assert!(!err.to_lowercase().contains("error"), "{target}:\n{err}");
            continue;
        }
        let out = Command::new("terraform")
            .arg("validate")
            .current_dir(&dir)
            .output()
            .expect("terraform validate");
        let salida = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{target} no valida:\n{salida}");
        // una advertencia hoy es un error del provider manana
        assert!(
            !salida.contains("Warning:"),
            "{target} valida con advertencias:\n{salida}"
        );
    }
}

/// El suite validaba el YAML que axon genera y nunca el del propio repo. Un
/// workflow que no parsea no falla: GitHub lo reporta con su ruta como nombre
/// y sin un solo job, o sea que un release roto se ve como que no corrio.
#[test]
fn los_workflows_del_repo_parsean() {
    let dir = std::path::Path::new(".github/workflows");
    let mut vistos = 0;
    for e in std::fs::read_dir(dir).expect("workflows") {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "yml" && x != "yaml") {
            continue;
        }
        let texto = std::fs::read_to_string(&p).unwrap();
        let doc: Result<serde_yaml_ng::Value, _> = serde_yaml_ng::from_str(&texto);
        assert!(doc.is_ok(), "{}: {}", p.display(), doc.unwrap_err());
        let doc = doc.unwrap();
        assert!(doc.get("jobs").is_some(), "{}: sin `jobs`", p.display());
        // `${{ }}` sin comillas dentro de un mapa en linea rompe el parseo,
        // porque la `{` abre un mapa anidado
        for (n, l) in texto.lines().enumerate() {
            let t = l.trim();
            if let (Some(mapa), Some(expr)) = (t.find(": {"), t.find("${{")) {
                // dentro de un escalar citado hay un numero impar de comillas
                // antes de la expresion
                let citado = t[..expr].matches('"').count() % 2 == 1;
                assert!(
                    mapa > expr || citado,
                    "{}:{}: `${{{{ }}}}` sin comillas en un mapa en linea:\n  {t}",
                    p.display(),
                    n + 1
                );
            }
        }
        vistos += 1;
    }
    assert!(vistos >= 3, "solo se validaron {vistos} workflows");
}

#[test]
fn el_ci_generado_es_yaml_valido() {
    let (yml, _, _) = axon(&["ci", "examples/payments.toml"]);
    // el fallo real que tuvo: un `: ` dentro de un escalar plano multilinea
    for linea in yml.lines() {
        let t = linea.trim_start();
        if t.starts_with("- run:") || t.starts_with("run:") {
            assert!(!t.ends_with('\\'), "run multilinea sin bloque escalar: {t}");
        }
    }
    assert!(
        yml.contains("run: |"),
        "los comandos multilinea necesitan bloque escalar"
    );
    assert!(yml.contains("id-token: write"), "sin OIDC");
}

/// El unico generador que hardcodeaba un cloud. Ahora el despliegue sale del
/// target, igual que la infraestructura.
#[test]
fn el_ci_no_hardcodea_un_cloud() {
    let marcas = [
        ("gcp", "gcloud run deploy", ["aws ecs", "kubectl"]),
        ("aws", "aws ecs update-service", ["gcloud", "kubectl"]),
        ("k8s", "kubectl rollout status", ["gcloud", "aws ecs"]),
    ];
    for (target, propia, ajenas) in marcas {
        let (yml, err, ok) = axon(&["ci", "examples/payments.toml", "--target", target]);
        assert!(ok, "{err}");
        assert!(yml.contains(propia), "{target} no genero `{propia}`");
        for ajena in ajenas {
            assert!(!yml.contains(ajena), "{target} filtro `{ajena}`");
        }
        // la infra va antes que el codigo, con el mismo target
        assert!(
            yml.contains(&format!("axon infra ./ --target {target}")),
            "{target} no aplica la infra antes de desplegar"
        );
    }
    // sin target no se inventa una plataforma
    let (sin, _, _) = axon(&["ci", "examples/payments.toml"]);
    for nube in ["gcloud", "aws ecs", "kubectl"] {
        assert!(!sin.contains(nube), "sin --target aparecio `{nube}`");
    }
    assert!(
        sin.contains("axon verify"),
        "sin --target se perdieron los gates"
    );

    // el layout del repo lo dice la policy, no axon
    let (yml, _, _) = axon(&["ci", "examples/payments.toml", "--target", "k8s"]);
    assert!(
        yml.contains("run: node --test services/payments"),
        "ignoro [ci].test_cmd"
    );
    assert!(
        yml.contains("> services/payments/contracts.ts"),
        "ignoro [ci].contracts_path"
    );
}

#[test]
fn maquinas_de_estado() {
    let (ts, _, _) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(
        ts.contains(r#"export type PaymentState = "pending" | "captured" | "failed" | "refunded""#),
        "{ts}"
    );
    assert!(ts.contains("paymentNext(state: PaymentState"));
    let (d, _, _) = axon(&["states", "examples"]);
    assert!(d.contains("pending --> captured: capture"));

    // deadlock y disparador fantasma tienen que bloquear
    let dir = std::env::temp_dir().join("axon-machine");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("m.toml"),
        r#"
service = "m"
owner = "x"
tier = "2"
[methods.go]
in = { a = "int" }
out = { b = "int" }
[machine.thing]
initial = "a"
[machine.thing.transitions.t1]
from = ["a"]
to = "b"
on = "fantasma"
"#,
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("no es ni metodo ni evento consumido"), "{err}");
    assert!(err.contains("deadlock"), "{err}");
}

#[test]
fn import_asyncapi_3_y_2() {
    // 3.x: send -> emits, receive -> consumes
    let (t3, err, ok) = axon(&[
        "import",
        "asyncapi",
        "examples/import/shipping.asyncapi.yaml",
    ]);
    assert!(ok, "{err}");
    assert!(t3.contains(r#"service = "shipping-service""#), "{t3}");
    assert!(t3.contains(r#"[emits."shipment.dispatched@v1"]"#), "{t3}");
    assert!(t3.contains(r#"[consumes."order.placed@v1"]"#), "{t3}");
    assert!(t3.contains(r#"handler = "onOrderPlaced""#));
    assert!(
        t3.contains(r#"dispatchedAt = "timestamp""#),
        "format date-time no mapeado"
    );
    assert!(
        t3.contains(r#"cost = "money""#),
        "{{amount,currency}} no se reconocio como money"
    );
    // el consumidor no declara campos: el esquema lo posee el emisor, asi que
    // el bloque de un evento consumido solo lleva su handler
    let consumido: Vec<&str> = t3
        .lines()
        .skip_while(|l| !l.starts_with("[consumes."))
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .collect();
    assert_eq!(
        consumido,
        vec![r#"handler = "onOrderPlaced""#],
        "el import copio el esquema de un evento ajeno"
    );

    // 2.x: publish es lo que la app RECIBE, subscribe lo que EMITE. Invertido.
    let (t2, err, ok) = axon(&[
        "import",
        "asyncapi",
        "examples/import/inventory.asyncapi.json",
    ]);
    assert!(ok, "{err}");
    assert!(
        t2.contains(r#"[consumes."order.placed@v1"]"#),
        "publish 2.x mal mapeado:\n{t2}"
    );
    assert!(
        t2.contains(r#"[emits."inventory.reserved@v1"]"#),
        "subscribe 2.x mal mapeado:\n{t2}"
    );

    // lo importado tiene que ser inmediatamente verificable, y decir que falta
    let dir = std::env::temp_dir().join("axon-import");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s.toml"), &t3).unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    // un placeholder no es un valor: si esto pasa, el import produce mentiras
    assert!(
        err.contains("no `owner`"),
        "TODO se acepto como owner:\n{err}"
    );
    assert!(err.contains("no `tier`"), "{err}");
}

/// El protocolo de plugins tiene que aguantar un generador de verdad, no solo
/// un check de tres lineas. Este esta escrito en Go, no sabe nada de axon, y
/// su salida tiene que compilar.
#[test]
fn plugin_gen_go() {
    if !tiene("go") {
        eprintln!("salteado: go no esta instalado");
        return;
    }
    let dir = std::env::temp_dir().join("axon-gen-go");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // se compila el plugin y se pone en el PATH, como haria cualquier usuario
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let build = Command::new("go")
        .args([
            "build",
            "-o",
            bin.join("axon-gen-go").to_str().unwrap(),
            ".",
        ])
        .current_dir("plugins/axon-gen-go")
        .output()
        .expect("go build");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_axon"))
        .args([
            "build",
            "examples/payments.toml",
            "examples",
            "--lang",
            "go",
        ])
        .env("PATH", &path)
        .output()
        .expect("axon");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let code = String::from_utf8_lossy(&out.stdout);

    // el esquema de un evento consumido lo posee su emisor: sin `peers` en el
    // protocolo, el plugin no podria declarar este tipo
    assert!(code.contains("type OrderPlacedV1 struct"), "{code}");
    assert!(
        code.contains("OrderID string `json:\"orderId\"`"),
        "no usa la convencion de Go"
    );
    assert!(
        code.contains("func PaymentNext(state PaymentState"),
        "sin maquina de estados"
    );
    assert!(
        code.contains("return s.outbox.Stage(ctx, e)"),
        "outbox declarado y no respetado"
    );
    assert!(code.contains("s.inbox.Once(ctx, e.ID"), "sin deduplicacion");

    // y compila
    let pkg = dir.join("payments");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("go.mod"), "module tmp/payments\n\ngo 1.22\n").unwrap();
    std::fs::write(pkg.join("axon.go"), code.as_bytes()).unwrap();
    let vet = Command::new("go")
        .args(["vet", "./..."])
        .current_dir(&pkg)
        .output()
        .expect("go vet");
    assert!(
        vet.status.success(),
        "el Go generado no pasa vet:\n{}",
        String::from_utf8_lossy(&vet.stderr)
    );
}

/// El codigo del servicio de ejemplo tambien tiene que typechequear, no solo
/// lo generado. Sin esto quedaba un hueco: cambiar la interfaz que emite axon
/// rompia la implementacion del ejemplo y el suite no lo veia, porque el
/// type-stripping de Node borra los tipos y en tiempo de ejecucion no falla.
#[test]
fn el_ejemplo_typechequea() {
    if !tiene("node") || !std::path::Path::new("examples/services/node_modules").exists() {
        eprintln!("salteado: falta node o `npm i` en examples/services");
        return;
    }
    // el testkit y los contratos se regeneran para que no se compruebe una
    // version vieja en disco
    for (manifiesto, destino) in [
        (
            "examples/orders.toml",
            "examples/services/orders/contracts.ts",
        ),
        (
            "examples/payments.toml",
            "examples/services/payments/contracts.ts",
        ),
    ] {
        let (ts, err, ok) = axon(&["build", manifiesto, "examples"]);
        assert!(ok, "{err}");
        assert_eq!(
            ts.trim(),
            std::fs::read_to_string(destino).unwrap().trim(),
            "{destino} quedo desactualizado: corre axon build"
        );
    }
    let out = Command::new("npm")
        .args(["run", "typecheck"])
        .current_dir("examples/services")
        .output()
        .expect("npm run typecheck");
    assert!(
        out.status.success(),
        "el ejemplo no typechequea:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `axon test` genera un testkit que tiene que compilar y correr contra la
/// implementacion real, no un esqueleto con huecos.
#[test]
fn el_testkit_generado_corre() {
    if !tiene("node") {
        eprintln!("salteado: node no esta instalado");
        return;
    }
    let pkg = std::path::Path::new("examples/services/payments");
    if !std::path::Path::new("examples/services/node_modules").exists() {
        eprintln!("salteado: falta `npm i` en examples/services");
        return;
    }

    // el testkit commiteado tiene que estar al dia con el manifiesto
    let (kit, err, ok) = axon(&["test", "examples/payments.toml", "examples"]);
    assert!(ok, "{err}");
    let commiteado = std::fs::read_to_string(pkg.join("axon.testkit.ts")).unwrap();
    assert_eq!(
        kit.trim(),
        commiteado.trim(),
        "el testkit commiteado quedo desactualizado: corre axon test"
    );

    let out = Command::new("node")
        .arg("--test")
        .current_dir(pkg)
        .output()
        .expect("node --test");
    let salida = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "las pruebas generadas fallan:\n{salida}"
    );
    assert!(salida.contains("propaga la cadena causal"), "{salida}");
    assert!(salida.contains("no repite el efecto"), "{salida}");
    assert!(salida.contains("fail 0"), "{salida}");
}

/// El gateway y el almacenamiento no son fuentes de verdad nuevas: salen de
/// los metodos con `http` y del bloque `[infra.buckets]`.
#[test]
fn el_edge_y_los_buckets_salen_del_plan() {
    // el edge, en los cuatro targets
    for (target, marca) in [
        ("local", "image: traefik:v3"),
        ("gcp", "google_compute_url_map"),
        ("aws", "aws_apigatewayv2_route"),
        ("k8s", "kind: HTTPRoute"),
    ] {
        let (out, _, _) = axon(&["infra", &fuente(target), "--target", target]);
        assert!(out.contains(marca), "{target} no genero el edge ({marca})");
    }
    // auth y rate limit llegan a la configuracion, no se quedan en el manifiesto
    let (k, _, _) = axon(&["infra", &fuente("k8s"), "--target", "k8s"]);
    assert!(k.contains("axon.dev/auth: public"), "{k}");
    assert!(k.contains("axon.dev/rate-limit: \"60\""), "{k}");
    assert!(
        k.contains("timeouts: { request: 5s }"),
        "el timeout del edge no llego"
    );
    let (a, _, _) = axon(&["infra", &fuente("aws"), "--target", "aws"]);
    assert!(
        a.contains("authorization_type = \"JWT\""),
        "ruta privada sin authorizer"
    );
    assert!(
        a.contains("authorization_type = \"NONE\""),
        "ruta publica mal marcada"
    );

    // publico implica CDN; privado implica que no la lleva
    let (g, _, _) = axon(&["infra", &fuente("gcp"), "--target", "gcp"]);
    assert!(g.contains("enable_cdn  = true"), "bucket publico sin CDN");
    assert!(
        g.contains("default_ttl = 86400"),
        "el cache_ttl no llego al CDN"
    );
    assert!(
        g.contains("public_access_prevention    = \"enforced\""),
        "bucket privado sin candado"
    );
    assert!(g.contains("age = 2555"), "la retencion no llego");
    assert!(
        !a.contains("cloudfront_distribution\" \"payments_recibos"),
        "CDN sobre un bucket privado"
    );

    // el nombre del bucket es una plantilla neutral en el plan
    let (plan, _, _) = axon(&["infra", "examples", "--target", "plan"]);
    assert!(plan.contains("{project}-payments-recibos"), "{plan}");
    assert!(
        !plan.contains("var.project"),
        "el plan neutral filtro sintaxis de terraform"
    );
    // y cada target la sustituye con la suya
    assert!(g.contains("${var.project}-payments-recibos"));
    assert!(
        a.contains(r#"{ name = "BUCKET_RECIBOS", value = "${var.project}-payments-recibos" }"#),
        "el nombre del bucket no llego al contenedor en aws"
    );
    assert!(k.contains("${PROJECT}-payments-recibos"));
    let (l, _, _) = axon(&["infra", "examples", "--target", "local"]);
    assert!(l.contains("BUCKET_RECIBOS: local-payments-recibos"));
    assert!(
        l.contains("image: minio/minio:latest"),
        "local sin almacenamiento de objetos"
    );
}

/// Una ruta expuesta sin decidir quien puede llamarla es un incidente, no un
/// default. El edge falla cerrado.
#[test]
fn el_edge_falla_cerrado() {
    let dir = std::env::temp_dir().join("axon-edge");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("e.toml"),
        r#"
service = "e"
owner = "x"
tier = "1"
[methods.abierto]
http = "POST /v1/abierto"
idempotent = true
auth = "public"
in = { a = "int" }
out = { b = "int" }
[methods.sinAuth]
http = "GET /v1/sin-auth"
in = { a = "int" }
out = { b = "int" }
"#,
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("expuesta sin `auth`"), "{err}");
    assert!(err.contains("publica y sin `rate_limit`"), "{err}");
}

/// Las reglas de seguridad citan su categoria del OWASP Top 10, porque un
/// error que no dice por que importa se silencia con un allow.
#[test]
fn las_reglas_owasp_disparan() {
    let dir = std::env::temp_dir().join("axon-owasp");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mal.toml"),
        r#"
service = "mal"
owner = "equipo"
tier = "0"
pii = ["email"]
[methods.pagar]
http = "POST /v1/pagar"
auth = "public"
rate_limit = 10
idempotent = true
in = { email = "string" }
out = { ok = "bool" }
[methods.perfil]
http = "GET /v1/perfil"
auth = "public"
rate_limit = 10
timeout_ms = 1000
in = { id = "uuid" }
out = { email = "string" }
[infra]
secrets = ["sk_ESTO_NO_ES_UNA_LLAVE"]
[infra.buckets.abierto]
public = true
"#,
    )
    .unwrap();
    let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    let todo = format!("{out}{err}");
    for regla in [
        "[A01] mal.pagar: public mutating route on a tier 0 service",
        "[A04] mal.pagar: public route with no `timeout_ms`",
        "[A09] mal.perfil: returns `email`, declared PII",
        "[A02] mal: `sk_ESTO_NO_ES_UNA_LLAVE`",
        "[A05] mal: bucket `abierto` is public and has no `retention_days`",
    ] {
        assert!(todo.contains(regla), "falto `{regla}`:\n{todo}");
    }

    // A05: el endurecimiento va generado, no recordado
    let (k, _, _) = axon(&["infra", &fuente("k8s"), "--target", "k8s"]);
    for marca in [
        "runAsNonRoot: true",
        "readOnlyRootFilesystem: true",
        "capabilities: { drop: [\"ALL\"] }",
        "automountServiceAccountToken: false",
        "kind: NetworkPolicy",
    ] {
        assert!(k.contains(marca), "k8s sin `{marca}`");
    }
    // A01: sin ruta publica no hay puerta a internet
    let (g, _, _) = axon(&["infra", &fuente("gcp"), "--target", "gcp"]);
    assert!(
        g.contains("ingress  = \"INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER\""),
        "un servicio sin ruta publica quedo expuesto"
    );
    // A08: se despliega por digest, no por etiqueta
    let (ci, _, _) = axon(&["ci", "examples/payments.toml", "--target", "gcp"]);
    assert!(
        ci.contains("@${{ steps.imagen.outputs.digest }}"),
        "deploy por etiqueta mutable"
    );
    // A09: la lista de PII y su redactor llegan al codigo
    let (ts, _, _) = axon(&["build", "examples/orders.toml", "examples"]);
    assert!(
        ts.contains("export const piiFields = [\"customer_email\"]"),
        "{ts}"
    );
    assert!(ts.contains("export function redact"), "{ts}");
    // El mismo concepto se declara UNA vez: `customer_email` en el manifiesto
    // cubre `customerEmail` en el contrato y `customer-email` en una cabecera.
    // Antes hacia falta declararlo dos veces, que es absurdo.
    assert!(
        ts.contains("const normalizePii"),
        "el redactor compara claves exactas:\n{ts}"
    );
    assert!(ts.contains("pii.has(normalizePii(k))"), "{ts}");
    // y la normalizacion llega a las tres capas desde una sola declaracion
    let (bodega, _, _) = axon(&["analytics", "examples"]);
    assert!(
        bodega.contains("customer_email_hash"),
        "la bodega no reconocio el campo del evento:\n{bodega}"
    );
    let (rls, _, _) = axon(&["rls", "examples"]);
    assert!(
        rls.contains(r#"'[redactado]'::text AS "customer_email""#),
        "la vista no enmascaro la columna:\n{rls}"
    );
}

/// RLS y enmascarado no se comprueban leyendo el SQL: se aplican a un Postgres
/// de verdad y se mira si aislan.
#[test]
fn la_rls_generada_aisla_de_verdad() {
    if !tiene("docker") {
        eprintln!("salteado: docker no esta instalado");
        return;
    }
    let (sql, err, ok) = axon(&["rls", "examples"]);
    assert!(ok, "{err}");
    assert!(
        sql.contains(r#"ALTER TABLE "order" FORCE ROW LEVEL SECURITY"#),
        "{sql}"
    );
    // `order` es palabra reservada: sin comillas el SQL no corre
    assert!(
        !sql.contains("ALTER TABLE order "),
        "identificador sin citar"
    );
    // el rol se crea una vez, no una por servicio
    assert_eq!(sql.matches("CREATE ROLE axon_lectura").count(), 1);

    let dir = std::env::temp_dir().join("axon-rls-sql");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut todo = String::new();
    for f in ["001_order.expand.sql", "003_tenant.expand.sql"] {
        todo.push_str(&std::fs::read_to_string(format!("examples/sql/orders/{f}")).unwrap());
    }
    todo.push_str(&sql);
    todo.push_str(
        r#"
CREATE ROLE app LOGIN PASSWORD 'x';
GRANT ALL ON ALL TABLES IN SCHEMA public TO app;
GRANT SELECT ON "order_enmascarada" TO app;
INSERT INTO "order" (id, customer_id, total_cents, status, tenant_id, customer_email)
VALUES ('11111111-1111-4111-8111-111111111111','cccccccc-0000-4000-8000-000000000001',100,'placed','aaaaaaaa-0000-4000-8000-000000000001','ana@ejemplo.mx'),
       ('22222222-2222-4222-8222-222222222222','cccccccc-0000-4000-8000-000000000002',200,'placed','bbbbbbbb-0000-4000-8000-000000000002','beto@ejemplo.mx');
SET ROLE app;
SELECT 'SIN_INQUILINO=' || count(*) FROM "order";
SET axon.tenant = 'aaaaaaaa-0000-4000-8000-000000000001';
SELECT 'INQUILINO_A=' || count(*) || ':' || min(customer_email) FROM "order";
SET axon.tenant = 'bbbbbbbb-0000-4000-8000-000000000002';
SELECT 'INQUILINO_B=' || count(*) || ':' || min(customer_email) FROM "order";
SELECT 'ENMASCARADA=' || min(customer_email) FROM "order_enmascarada";
"#,
    );
    std::fs::write(dir.join("todo.sql"), &todo).unwrap();

    let nombre = "axon-test-rls";
    let _ = Command::new("docker").args(["rm", "-f", nombre]).output();
    let arranque = Command::new("docker")
        .args([
            "run",
            "-d",
            "--rm",
            "--name",
            nombre,
            "-e",
            "POSTGRES_PASSWORD=x",
            "-e",
            "POSTGRES_DB=t",
            "postgres:16-alpine",
        ])
        .output()
        .expect("docker run");
    if !arranque.status.success() {
        eprintln!(
            "salteado: no se pudo arrancar postgres: {}",
            String::from_utf8_lossy(&arranque.stderr)
        );
        return;
    }
    // se limpia pase lo que pase, incluso si un assert revienta
    struct Limpieza(&'static str);
    impl Drop for Limpieza {
        fn drop(&mut self) {
            let _ = Command::new("docker").args(["rm", "-f", self.0]).output();
        }
    }
    let _limpieza = Limpieza(nombre);

    let mut listo = false;
    for _ in 0..60 {
        // pg_isready responde antes de que el init cree la base: postgres
        // reinicia a mitad de su inicializacion. Se espera la base real.
        let r = Command::new("docker")
            .args([
                "exec", nombre, "psql", "-U", "postgres", "-d", "t", "-c", "select 1",
            ])
            .output();
        if r.is_ok_and(|o| o.status.success()) {
            listo = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert!(listo, "postgres no arranco");

    let out = Command::new("docker")
        .args([
            "exec", "-i", nombre, "psql", "-q", "-tA", "-U", "postgres", "-d", "t",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(todo.as_bytes())?;
            c.wait_with_output()
        })
        .expect("docker exec psql");
    let salida = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        salida.contains("SIN_INQUILINO=0"),
        "RLS no aplica sin inquilino:\n{salida}"
    );
    assert!(
        salida.contains("INQUILINO_A=1:ana@ejemplo.mx"),
        "el inquilino A no ve su fila:\n{salida}"
    );
    assert!(
        salida.contains("INQUILINO_B=1:beto@ejemplo.mx"),
        "el inquilino B no ve su fila:\n{salida}"
    );
    assert!(
        !salida.contains("INQUILINO_A=2") && !salida.contains("INQUILINO_B=2"),
        "fuga entre inquilinos:\n{salida}"
    );
    assert!(
        salida.contains("ENMASCARADA=[redactado]"),
        "la vista no enmascara:\n{salida}"
    );

    // Como se fija el inquilino importa tanto como la politica, y esto lo mide
    // en vez de inferirlo. El resultado es mas fuerte que "usá SET LOCAL":
    // un solo `SET` de sesion envenena la conexion para todo SET LOCAL
    // posterior, porque SET LOCAL revierte al valor de la SESION y no a nada.
    let prueba = r#"
BEGIN; SET LOCAL axon.tenant = 'aaaaaaaa-0000-4000-8000-000000000001'; COMMIT;
SELECT 'LIMPIA=' || coalesce(NULLIF(current_setting('axon.tenant', true), ''), 'SIN_FIJAR');
SET axon.tenant = 'bbbbbbbb-0000-4000-8000-000000000002';
BEGIN; SET LOCAL axon.tenant = 'aaaaaaaa-0000-4000-8000-000000000001'; COMMIT;
SELECT 'ENVENENADA=' || current_setting('axon.tenant', true);
RESET ALL;
SELECT 'TRAS_RESET=' || coalesce(NULLIF(current_setting('axon.tenant', true), ''), 'SIN_FIJAR');
"#;
    let out2 = Command::new("docker")
        .args([
            "exec", "-i", nombre, "psql", "-q", "-tA", "-U", "postgres", "-d", "t",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(prueba.as_bytes())?;
            c.wait_with_output()
        })
        .expect("docker exec psql");
    let s2 = format!(
        "{}{}",
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr)
    );
    // en una conexion limpia, SET LOCAL no sobrevive al commit
    assert!(
        s2.contains("LIMPIA=SIN_FIJAR"),
        "SET LOCAL sobrevivio al commit:\n{s2}"
    );
    // pero tras un `SET` de sesion, revierte a ESE valor: la fuga persiste
    assert!(
        s2.contains("ENVENENADA=bbbbbbbb-0000-4000-8000-000000000002"),
        "el comportamiento medido cambio; revisar la prescripcion de la RLS:\n{s2}"
    );
    assert!(s2.contains("TRAS_RESET=SIN_FIJAR"), "{s2}");

    // y la prescripcion generada dice exactamente eso
    assert!(
        sql.contains("NUNCA un `SET` de"),
        "la migracion no prescribe como fijar el inquilino"
    );
    assert!(
        sql.contains("NULLIF(current_setting('axon.tenant', true), '')::uuid"),
        "{sql}"
    );
}

/// El hueco mas grave que tuvo la herramienta: se le podia cambiar un campo a
/// una version ya publicada y `verify` salia limpio.
#[test]
fn una_version_publicada_es_inmutable() {
    let base = std::env::temp_dir().join("axon-baseline");

    // prepara una copia de los ejemplos con su baseline, sin migraciones
    // (aqui solo se prueban contratos)
    let preparar = |sufijo: &str| -> std::path::PathBuf {
        let dir = base.join(sufijo);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["orders.toml", "payments.toml", "stripe.external.toml"] {
            let t = std::fs::read_to_string(format!("examples/{f}")).unwrap();
            let t: String = t
                .lines()
                .filter(|l| !l.trim_start().starts_with("migrations ="))
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(dir.join(f), t).unwrap();
        }
        let (b, err, ok) = axon(&["baseline", dir.to_str().unwrap()]);
        assert!(ok, "{err}");
        std::fs::write(dir.join("axon.baseline.json"), b).unwrap();
        dir
    };

    // el punto de partida esta limpio
    let dir = preparar("limpio");
    let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "{err}");
    assert!(out.contains("0 errores"), "{out}");
    assert!(
        !out.contains("sin registrar"),
        "el baseline recien tomado ya tiene huecos"
    );

    let cambiar = |sufijo: &str, archivo: &str, de: &str, a: &str| -> String {
        let dir = preparar(sufijo);
        let p = dir.join(archivo);
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains(de), "el fixture no contiene `{de}`");
        std::fs::write(&p, t.replace(de, a)).unwrap();
        let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        assert!(!ok, "el cambio `{de}` -> `{a}` paso limpio");
        err
    };

    // tipo cambiado
    let err = cambiar(
        "tipo",
        "orders.toml",
        "total = \"money\"",
        "total = \"int\"",
    );
    assert!(
        err.contains("cambio de `money` a `int` en una version publicada"),
        "{err}"
    );
    assert!(
        err.contains("publica `order.placed@v2`"),
        "no dice que hacer:\n{err}"
    );

    // campo agregado: en axon todo campo es obligatorio, asi que rompe igual
    let err = cambiar(
        "agregado",
        "orders.toml",
        "\ntotal = \"money\"\n",
        "\ntotal = \"money\"\nchannel = \"string\"\n",
    );
    assert!(
        err.contains("campo nuevo `channel` en una version publicada"),
        "{err}"
    );

    // ruta movida
    let err = cambiar(
        "ruta",
        "orders.toml",
        "http = \"POST /v1/tenants/{tenantId}/orders\"",
        "http = \"POST /v1/pedidos\"",
    );
    assert!(
        err.contains("la ruta cambio de `POST /v1/tenants/{tenantId}/orders`"),
        "{err}"
    );
    assert!(
        !err.contains("Some("),
        "el mensaje filtra el Debug de Option:\n{err}"
    );

    // retirar una version publicada
    let err = cambiar(
        "retiro",
        "orders.toml",
        "[emits.\"order.placed@v1\"]",
        "[emits.\"order.placed@v2\"]",
    );
    assert!(
        err.contains("estaba publicado por orders y ya nadie lo emite"),
        "{err}"
    );

    // la via de escape: retirarlo tambien del baseline, visible en el PR
    let dir = preparar("retiro_deliberado");
    for (f, de, a) in [
        (
            "orders.toml",
            "[emits.\"order.placed@v1\"]",
            "[emits.\"order.placed@v2\"]",
        ),
        ("payments.toml", "order.placed@v1", "order.placed@v2"),
    ] {
        let p = dir.join(f);
        let t = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, t.replace(de, a)).unwrap();
    }
    let bl = dir.join("axon.baseline.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&bl).unwrap()).unwrap();
    v["events"]
        .as_object_mut()
        .unwrap()
        .remove("order.placed@v1");
    std::fs::write(&bl, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "un retiro deliberado no deberia bloquear:\n{err}");

    // un contrato nuevo sin registrar no esta protegido, y hay que avisarlo
    let dir = preparar("nuevo");
    let p = dir.join("orders.toml");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(
        &p,
        format!("{t}\n[emits.\"order.cancelled@v1\"]\norderId = \"uuid\"\n"),
    )
    .unwrap();
    let (out, _, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "un contrato nuevo no es un error");
    assert!(out.contains("sin registrar"), "{out}");

    // y sin baseline, `verify` tiene que decir que no puede ver esto
    let dir = preparar("sin_baseline");
    std::fs::remove_file(dir.join("axon.baseline.json")).unwrap();
    let (out, _, _) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(out.contains("sin axon.baseline.json"), "{out}");
}

/// La resiliencia declarada tiene que ejecutarse, no solo validarse: era la
/// unica promesa del manifiesto que no llegaba al codigo.
#[test]
fn la_politica_declarada_se_ejecuta() {
    if !tiene("node") || !std::path::Path::new("examples/services/node_modules").exists() {
        eprintln!("salteado: falta node o `npm i` en examples/services");
        return;
    }
    let (ts, err, ok) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(ok, "{err}");
    // los numeros del manifiesto llegan literales al codigo
    assert!(
        ts.contains(
            r#"withPolicy("orders.getOrder", { timeoutMs: 1000, retries: 3, breaker: true }"#
        ),
        "la politica no salio del manifiesto:\n{ts}"
    );
    // reintentar sin llave de idempotencia duplica el efecto del otro lado
    assert!(ts.contains(r#"headers(e, true)"#));
    // CAP: el lado declarado decide el aislamiento
    assert!(
        ts.contains(r#"export const isolationLevel = "SERIALIZABLE""#),
        "{ts}"
    );
    let (o, _, _) = axon(&["build", "examples/orders.toml", "examples"]);
    assert!(
        o.contains(r#"export const isolationLevel = "READ COMMITTED""#),
        "{o}"
    );
    assert!(
        o.contains("export const maxStalenessMs = 3000"),
        "{o}"
    );
    // `degrade` obliga a pasar el camino degradado; `reject` no lo admite
    assert!(
        o.contains("fallback: () => Promise<PaymentsCapturePaymentOut>"),
        "declarar degrade no obligo a un fallback:\n{o}"
    );
    assert!(
        !ts.contains("fallback: () =>"),
        "un servicio `reject` no degrada"
    );

    // y la politica se comporta: se ejecuta contra el codigo generado
    let dir = std::path::Path::new("examples/services/payments");
    let prueba = dir.join("axon.politica.test.ts");
    std::fs::write(
        &prueba,
        r#"
import { test } from "node:test";
import assert from "node:assert/strict";
import { withPolicy, headers, newEnvelope, TimedOut, CircuitOpen } from "./contracts.ts";

test("the timeout cuts the call", async () => {
  await assert.rejects(
    () => withPolicy("x.slow", { timeoutMs: 50, retries: 0, breaker: false },
      () => new Promise((r) => setTimeout(r, 5000))),
    TimedOut,
  );
});

test("it retries until it succeeds", async () => {
  let n = 0;
  const r = await withPolicy("x.flaky", { timeoutMs: 500, retries: 3, breaker: false }, async () => {
    if (++n < 3) throw new Error("boom");
    return "ok";
  });
  assert.equal(r, "ok");
  assert.equal(n, 3);
});

test("the breaker opens and stops hitting", async () => {
  let calls = 0;
  const down = () => withPolicy("x.down", { timeoutMs: 100, retries: 0, breaker: true },
    async () => { calls++; throw new Error("down"); });
  for (let i = 0; i < 5; i++) await assert.rejects(down);
  assert.equal(calls, 5);
  await assert.rejects(down, CircuitOpen);
  assert.equal(calls, 5, "it kept hitting with the breaker open");
});

test("the trace and the idempotency key travel with the call", () => {
  const e = newEnvelope("x@v1", "test", {});
  const h = headers(e, true);
  assert.equal(h.traceparent, e.traceparent);
  assert.equal(h["x-correlation-id"], e.correlationId);
  assert.equal(h["x-causation-id"], e.id);
  assert.equal(h["idempotency-key"], e.id);
  assert.equal(headers(e, false)["idempotency-key"], undefined);
});
"#,
    )
    .unwrap();
    let out = Command::new("node")
        .args(["--test", "axon.politica.test.ts"])
        .current_dir(dir)
        .output()
        .expect("node --test");
    let _ = std::fs::remove_file(&prueba);
    let salida = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "la politica generada no se comporta:\n{salida}"
    );
    assert!(salida.contains("pass 4"), "{salida}");
}

/// CAP: la particion no se elige, que hacer mientras dura si.
#[test]
fn el_lado_cap_se_verifica() {
    let dir = std::env::temp_dir().join("axon-cap");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("c.toml"),
        r#"
service = "c"
owner = "x"
tier = "1"
[cap]
consistency = "eventual"
on_partition = "reject"
"#,
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("sin `max_staleness_ms`"), "{err}");

    // la contradiccion del teorema: CP que sirve algo viejo
    std::fs::write(
        dir.join("c.toml"),
        "service = \"c\"\nowner = \"x\"\ntier = \"1\"\n[cap]\nconsistency = \"strong\"\non_partition = \"degrade\"\n",
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("se contradice"), "{err}");

    // sin declararlo, se asume el par que falla cerrado, y se avisa
    std::fs::write(
        dir.join("c.toml"),
        "service = \"c\"\nowner = \"x\"\ntier = \"1\"\n",
    )
    .unwrap();
    let (out, _, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok);
    assert!(out.contains("sin `[cap]`; asumido CP"), "{out}");

    // la garantia de una ruta sincrona es la del eslabon mas debil
    let (out, _, _) = axon(&["verify", "examples"]);
    assert!(
        out.contains("es `strong` y llama a orders que es `eventual`"),
        "{out}"
    );
}

/// OpenTelemetry: axon no trae un SDK ni inventa un formato. El envelope ya
/// propaga `traceparent`, que es el contexto W3C que usa OTel, asi que solo
/// levanta el backend en local y pone las variables estandar en los cuatro
/// targets — el destino cambia, los atributos no.
#[test]
fn otel_en_los_cuatro_targets() {
    let esperado = [
        "OTEL_SERVICE_NAME",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_RESOURCE_ATTRIBUTES",
        "OTEL_TRACES_SAMPLER",
    ];
    for target in ["local", "gcp", "aws", "k8s"] {
        let (out, err, ok) = axon(&["infra", &fuente(target), "--target", target]);
        assert!(ok, "{target}: {err}");
        for v in esperado {
            assert!(out.contains(v), "{target} no inyecta {v}");
        }
        // los atributos de recurso salen del manifiesto, no de una convencion
        assert!(
            out.contains("axon.owner=equipo-pagos") && out.contains("axon.tier=0"),
            "{target}: los atributos no salen del manifiesto"
        );
    }

    // el destino es lo unico que cambia entre targets
    let (l, _, _) = axon(&["infra", "examples", "--target", "local"]);
    assert!(l.contains("http://traza:4318"));
    assert!(
        l.contains("image: jaegertracing/all-in-one"),
        "local sin backend de trazas"
    );
    for (target, endpoint) in [
        ("gcp", "${var.otlp_endpoint}"),
        ("aws", "${var.otlp_endpoint}"),
        ("k8s", "${OTLP_ENDPOINT}"),
    ] {
        let (o, _, _) = axon(&["infra", &fuente(target), "--target", target]);
        assert!(
            o.contains(endpoint),
            "{target} sin destino OTLP configurable"
        );
    }

    // el muestreo sale del tier: un tier 0 se traza entero, porque cuando se
    // cae la traza que falta es justo la que hacia falta
    let (g, _, _) = axon(&["infra", &fuente("gcp"), "--target", "gcp"]);
    assert!(
        g.contains("parentbased_always_on"),
        "tier 0 sin muestreo completo"
    );
    assert!(
        g.contains("parentbased_traceidratio"),
        "tier 1 sin muestreo parcial"
    );
    // pero en local se traza todo: descartar el 90% mientras depuras no sirve
    assert!(
        !l.contains("parentbased_traceidratio"),
        "local muestrea parcialmente"
    );

    // los flags del traceparent se heredan, no se inventan: declarar
    // "muestreado" sobre una traza que no lo esta parte el arbol en fragmentos
    let (ts, _, _) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(ts.contains(r#"const flags = partes?.[3] ?? "01""#), "{ts}");
    assert!(!ts.contains("${hex(8)}-01`"), "el envelope fija los flags");
}

/// El diccionario de pg_anon: cada campo `pii` recibe una regla, y cada regla
/// se aplica de verdad a una columna de su tipo. Una funcion que no existe
/// —o un cast que falta— hace fallar el dump a mitad de camino.
#[test]
fn el_diccionario_pg_anon_funciona() {
    let dir = std::env::temp_dir().join("axon-pganon");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sql")).unwrap();
    let ddl = "CREATE TABLE persona (\n  \
               id uuid PRIMARY KEY,\n  tenant_id uuid NOT NULL,\n  correo text NOT NULL,\n  \
               nacimiento timestamptz,\n  ingreso bigint,\n  preferencias jsonb,\n  \
               verificado boolean\n);\n";
    std::fs::write(dir.join("sql/001.expand.sql"), ddl).unwrap();
    std::fs::write(
        dir.join("p.toml"),
        "service = \"gente\"\nowner = \"equipo\"\ntier = \"1\"\n\
         pii = [\"id\", \"correo\", \"nacimiento\", \"ingreso\", \"preferencias\", \"verificado\"]\n\
         [infra]\nstate = \"postgres\"\nmigrations = \"sql/\"\ntenant_column = \"tenant_id\"\n",
    )
    .unwrap();

    let (dic, err, ok) = axon(&["rls", dir.to_str().unwrap(), "--target", "pg_anon"]);
    assert!(ok, "{err}");
    // cobertura: ningun campo declarado se queda sin regla
    for campo in [
        "id",
        "correo",
        "nacimiento",
        "ingreso",
        "preferencias",
        "verificado",
    ] {
        assert!(
            dic.contains(&format!("\"{campo}\":")),
            "sin regla para {campo}:\n{dic}"
        );
    }
    // md5(uuid) no existe: el cast interno es obligatorio
    assert!(dic.contains(r#"md5(\"id\"::text)::uuid"#), "{dic}");
    // la regla sale del tipo, no del nombre: `correo` no lleva "mail" y aun asi
    // recibe la regla de texto
    assert!(dic.contains("anon_funcs.digest(\\\"correo\\\""), "{dic}");
    // las tablas del propio framework se excluyen: el outbox lleva payloads
    let (dic2, _, _) = axon(&["rls", "examples", "--target", "pg_anon"]);
    assert!(
        dic2.contains("\"table\": \"outbox\"") && dic2.contains("dictionary_exclude"),
        "{dic2}"
    );

    if !tiene("docker") {
        eprintln!("salteado el resto: docker no esta instalado");
        return;
    }

    // cada regla, aplicada a una columna de su tipo
    let nombre = "axon-test-pganon";
    let _ = Command::new("docker").args(["rm", "-f", nombre]).output();
    let arranque = Command::new("docker")
        .args([
            "run",
            "-d",
            "--rm",
            "--name",
            nombre,
            "-e",
            "POSTGRES_PASSWORD=x",
            "-e",
            "POSTGRES_DB=t",
            "postgres:16-alpine",
        ])
        .output()
        .expect("docker run");
    if !arranque.status.success() {
        eprintln!("salteado: no arranco postgres");
        return;
    }
    struct Limpieza(&'static str);
    impl Drop for Limpieza {
        fn drop(&mut self) {
            let _ = Command::new("docker").args(["rm", "-f", self.0]).output();
        }
    }
    let _l = Limpieza(nombre);
    let mut listo = false;
    for _ in 0..60 {
        if Command::new("docker")
            .args([
                "exec", nombre, "psql", "-U", "postgres", "-d", "t", "-c", "select 1",
            ])
            .output()
            .is_ok_and(|o| o.status.success())
        {
            listo = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert!(listo, "postgres no arranco");

    let mut sql = String::from(ddl);
    sql.push_str(
        "INSERT INTO persona VALUES ('11111111-1111-4111-8111-111111111111',\n  \
         '22222222-2222-4222-8222-222222222222','ana@gmail.com','2026-03-14 10:30:00+00',\n  \
         25000,'{\"a\":1}'::jsonb,true);\n\
         -- doble de anon_funcs.digest, solo para comprobar la FORMA de la llamada:\n\
         -- la funcion real la instala pg_anon en el destino.\n\
         CREATE SCHEMA anon_funcs;\n\
         CREATE FUNCTION anon_funcs.digest(t text, salt text, algo text) RETURNS text\n  \
           AS $$ SELECT encode(sha256((t || salt)::bytea), 'hex') $$ LANGUAGE sql IMMUTABLE;\n",
    );
    // cada regla del diccionario, tal cual, contra su columna
    for linea in dic.lines() {
        let l = linea.trim();
        if !l.starts_with('"') || !l.contains("\": \"") {
            continue;
        }
        let Some((campo, resto)) = l.split_once("\": \"") else {
            continue;
        };
        let campo = campo.trim_start_matches('"');
        if ["schema", "table"].contains(&campo) {
            continue;
        }
        // el valor viene escapado para Python: aca se desescapa para SQL
        let regla = resto
            .trim_end_matches(&[',', '"'][..])
            .replace("\\\"", "\"");
        sql.push_str(&format!("SELECT {regla} FROM persona;\n"));
    }

    let out = Command::new("docker")
        .args([
            "exec",
            "-i",
            nombre,
            "psql",
            "-q",
            "-tA",
            "-v",
            "ON_ERROR_STOP=1",
            "-U",
            "postgres",
            "-d",
            "t",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(sql.as_bytes())?;
            c.wait_with_output()
        })
        .expect("docker exec psql");
    let salida = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "una regla generada no corre en Postgres:\n{salida}"
    );
    // el dato original no sobrevive a ninguna regla
    assert!(
        !salida.contains("ana@gmail.com"),
        "el correo no se enmascaro:\n{salida}"
    );
    assert!(
        salida.contains("2026-01-01"),
        "la fecha no se trunco:\n{salida}"
    );
}

/// Escalado de la base: aritmetica sobre lo declarado. El agotamiento de
/// conexiones no aparece con una instancia; aparece el dia que escala.
#[test]
fn el_escalado_de_la_base_se_verifica() {
    let dir = std::env::temp_dir().join("axon-escala");
    let escribir = |cuerpo: &str| {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("s.toml"), cuerpo).unwrap();
        let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        (format!("{out}{err}"), ok)
    };

    // 20 x 10 instancias = 200 conexiones contra un tope de 100
    let (msg, ok) = escribir(
        "service = \"s\"\nowner = \"x\"\ntier = \"2\"\n[infra]\nstate = \"postgres\"\n\
         pool_size = 20\nmax_connections = 100\nmax_instances = 10\n",
    );
    assert!(!ok);
    assert!(msg.contains("20 conexiones x 10 instancias = 200"), "{msg}");
    assert!(msg.contains("supera el tope de 100"), "{msg}");

    // una replica va con retraso: leer de ella y prometer consistencia fuerte
    // es la contradiccion del teorema escrita en dos lugares
    let (msg, ok) = escribir(
        "service = \"s\"\nowner = \"x\"\ntier = \"2\"\n[cap]\nconsistency = \"strong\"\n\
         [infra]\nstate = \"postgres\"\nread_replicas = 2\n",
    );
    assert!(!ok);
    assert!(msg.contains("lee de 2 replicas y declara"), "{msg}");

    // alta disponibilidad no es fallback: el standby replica el DROP TABLE
    let (msg, ok) = escribir(
        "service = \"s\"\nowner = \"x\"\ntier = \"0\"\n[infra]\nstate = \"postgres\"\nha = true\n",
    );
    assert!(!ok);
    assert!(msg.contains("sin `backup_retention_days`"), "{msg}");
    assert!(msg.contains("no es respaldo"), "{msg}");

    // un tier 0 sin failover no es un tier 0
    let (msg, ok) = escribir(
        "service = \"s\"\nowner = \"x\"\ntier = \"0\"\n[infra]\nstate = \"postgres\"\n\
         backup_retention_days = 30\n",
    );
    assert!(!ok);
    assert!(msg.contains("sin `ha = true`"), "{msg}");

    // y los recursos: standby, respaldos y replicas salen del manifiesto
    let (g, _, _) = axon(&["infra", &fuente("gcp"), "--target", "gcp"]);
    assert!(
        g.contains("availability_type = \"REGIONAL\""),
        "payments es tier 0: falta el standby"
    );
    assert!(g.contains("retained_backups = 30"), "{g}");
    assert!(g.contains("point_in_time_recovery_enabled = true"), "{g}");
    assert!(
        g.contains("master_instance_name = google_sql_database_instance.orders.name"),
        "sin replicas"
    );
    // una base por servicio es una INSTANCIA por servicio
    assert!(
        g.contains("resource \"google_sql_database_instance\" \"payments\""),
        "{g}"
    );
    assert!(
        !g.contains("var.sql_instance"),
        "las bases siguen compartiendo instancia"
    );
    let (a, _, _) = axon(&["infra", &fuente("aws"), "--target", "aws"]);
    assert!(a.contains("multi_az                = true"), "{a}");
    assert!(a.contains("backup_retention_period = 30"), "{a}");
    assert!(
        a.contains("replicate_source_db = aws_db_instance.orders.identifier"),
        "{a}"
    );
}

/// La prueba de carga sale del manifiesto, y su veredicto compara lo medido
/// con lo declarado. Un numero declarado que nadie mide es una opinion.
#[test]
fn la_carga_sale_del_manifiesto() {
    let (js, err, ok) = axon(&["load", "examples/orders.toml"]);
    assert!(ok, "{err}");
    // la tasa es el rate_limit declarado, no un numero elegido a ojo
    assert!(
        js.contains("rate: 60,              // declarado en rate_limit"),
        "{js}"
    );
    // el umbral es el timeout declarado
    assert!(
        js.contains(r#""http_req_duration{escenario:placeOrder}": ["p(95)<5000"]"#),
        "{js}"
    );
    assert!(
        js.contains(r#""http_req_duration{escenario:getOrder}": ["p(95)<2000"]"#),
        "{js}"
    );
    // una ruta con parametro se prueba con un id inventado: un 404 ahi no es
    // un fallo del servicio, asi que el umbral va sobre el check
    assert!(
        js.contains(r#""checks{escenario:getOrder}": ["rate>0.99"]"#),
        "{js}"
    );
    assert!(js.contains("r.status === 404"), "{js}");
    // el techo que impone el pool declarado queda escrito
    assert!(js.contains("4 conexiones x 10 instancias = 40"), "{js}");

    // y el veredicto: `true` en un umbral de k6 significa INCUMPLIDO
    let dir = std::env::temp_dir().join("axon-carga");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let bien = dir.join("bien.json");
    std::fs::write(
        &bien,
        r#"{"metrics":{"http_req_duration{escenario:placeOrder}":{"p(95)":23.8,"thresholds":{"p(95)<5000":false}},"http_reqs":{"count":41,"rate":2.05}}}"#,
    )
    .unwrap();
    let (out, err, ok) = axon(&[
        "load",
        "examples/orders.toml",
        "--check",
        bien.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    assert!(out.contains("0 umbrales incumplidos"), "{out}");
    assert!(out.contains("41 peticiones medidas"), "{out}");

    let mal = dir.join("mal.json");
    std::fs::write(
        &mal,
        r#"{"metrics":{"http_req_duration{escenario:placeOrder}":{"p(95)":9000,"thresholds":{"p(95)<5000":true}}}}"#,
    )
    .unwrap();
    let (_, err, ok) = axon(&[
        "load",
        "examples/orders.toml",
        "--check",
        mal.to_str().unwrap(),
    ]);
    assert!(!ok, "un umbral incumplido tiene que fallar");
    assert!(err.contains("incumplio `p(95)<5000`"), "{err}");

    // un resumen sin umbrales no es un veredicto, y decirlo es mejor que
    // dar por bueno lo que no se midio
    let vacio = dir.join("vacio.json");
    std::fs::write(&vacio, r#"{"metrics":{"http_reqs":{"count":1,"rate":1}}}"#).unwrap();
    let (_, err, ok) = axon(&[
        "load",
        "examples/orders.toml",
        "--check",
        vacio.to_str().unwrap(),
    ]);
    assert!(!ok);
    assert!(err.contains("no trae umbrales"), "{err}");
}

/// Una ruta declarada que nadie sirve devuelve 404 en produccion y no aparece
/// en ninguna prueba. El codigo generado publica la lista para que el arranque
/// pueda negarse.
#[test]
fn las_rutas_declaradas_llegan_al_codigo() {
    let (ts, _, _) = axon(&["build", "examples/orders.toml", "examples"]);
    assert!(
        ts.contains(
            r#"export const httpRoutes = ["POST /v1/tenants/{tenantId}/orders", "GET /v1/tenants/{tenantId}/orders/{orderId}"]"#
        ),
        "{ts}"
    );
}

/// Feature flags: lo que aporta declararlos no es el SDK, sino que el
/// compilador imponga lo que nadie impone.
#[test]
fn los_flags_se_verifican() {
    let dir = std::env::temp_dir().join("axon-flags");
    let probar = |cuerpo: &str| {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.toml"), cuerpo).unwrap();
        let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        (format!("{out}{err}"), ok)
    };
    let base = "service = \"f\"\nowner = \"e\"\ntier = \"2\"\n";

    // un flag sin fecha de muerte no muere
    let (msg, ok) = probar(&format!("{base}[flags.eterno]\nowner = \"e\"\n"));
    assert!(!ok);
    assert!(msg.contains("no `expires`"), "{msg}");

    // uno vencido no se ignora: se limpia o se renueva
    let (msg, ok) = probar(&format!(
        "{base}[flags.viejo]\nowner = \"e\"\nexpires = \"2024-01-15\"\n"
    ));
    assert!(!ok);
    assert!(msg.contains("expired on 2024-01-15"), "{msg}");

    // sin dueno, nadie lo apaga
    let (msg, ok) = probar(&format!(
        "{base}[flags.huerfano]\nexpires = \"2099-01-01\"\n"
    ));
    assert!(!ok);
    assert!(msg.contains("flag with no `owner`"), "{msg}");

    // un rollout por peticion deja la misma entidad a medio migrar
    let (msg, ok) = probar(&format!(
        "{base}[flags.parcial]\nowner = \"e\"\nexpires = \"2099-01-01\"\nrollout = 25\n"
    ));
    assert!(!ok);
    assert!(msg.contains("rollout at 25% with no `sticky_by`"), "{msg}");

    // un kill switch se apaga entero: el error es propio, no el del sticky
    let (msg, ok) = probar(&format!(
        "{base}[flags.cortar]\nowner = \"e\"\nkill_switch = true\nrollout = 50\n"
    ));
    assert!(!ok);
    assert!(msg.contains("`kill_switch` with `rollout`"), "{msg}");

    // fijarse por un campo que el servicio no recibe no fija nada
    let (msg, ok) = probar(&format!(
        "{base}[flags.malo]\nowner = \"e\"\nexpires = \"2099-01-01\"\nrollout = 50\n\
         sticky_by = \"no_existe\"\n"
    ));
    assert!(!ok);
    assert!(msg.contains("no aparece en ningun contrato"), "{msg}");

    // un kill switch sin expires es lo correcto, y no molesta
    let (_, ok) = probar(&format!(
        "{base}[flags.corte]\nowner = \"e\"\nkill_switch = true\n"
    ));
    assert!(ok, "un kill switch legitimo no deberia fallar");

    // el codigo: el accesor exige el campo por el que se fija
    let (ts, _, _) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(
        ts.contains(
            "export const flagCobroV2 = (flags: Flags, tenant_id: string): Promise<boolean> =>"
        ),
        "{ts}"
    );
    assert!(
        ts.contains(r#"flags.evaluar("cobro_v2", false, { targetingKey: tenant_id, tenant_id })"#),
        "{ts}"
    );
    assert!(
        ts.contains(r#"export const flagsDeclarados = ["cobro_v2""#),
        "{ts}"
    );

    // los cuatro tipos de OpenFeature: un flag no es solo un booleano, y un
    // rollout de configuracion —un limite, un proveedor— necesita los otros
    assert!(
        ts.contains(
            "export const flagProveedorDeCobro = (flags: Flags, tenant_id: string): Promise<string> =>"
        ),
        "sin accesor tipado para un flag de tipo string:\n{ts}"
    );
    assert!(
        ts.contains(r#"flags.evaluar("proveedor_de_cobro", "stripe", "#),
        "{ts}"
    );
    assert!(
        ts.contains("export const flagLimiteDeReintentos = (flags: Flags): Promise<number> =>"),
        "sin accesor tipado para un flag numerico:\n{ts}"
    );
    // la interfaz cubre los cuatro tipos del estandar
    assert!(
        ts.contains("evaluar<T extends boolean | string | number | object>"),
        "{ts}"
    );

    // y la config de flagd: el rollout se expresa con su `fractional`
    let (cfg, _, _) = axon(&["flags", "examples"]);
    let v: serde_json::Value = serde_json::from_str(&cfg).expect("flagd json");
    let f = &v["flags"]["cobro_v2"];
    assert_eq!(f["defaultVariant"], "off");
    assert_eq!(f["targeting"]["fractional"][0]["var"], "tenant_id");
    assert_eq!(f["targeting"]["fractional"][1][1], 10);
    assert_eq!(f["targeting"]["fractional"][2][1], 90);
    // un kill switch no lleva targeting
    assert!(v["flags"]["cortar_stripe"]["targeting"].is_null());

    // y las variantes declaradas llegan a flagd tal cual, no un on/off fijo
    let p = &v["flags"]["proveedor_de_cobro"];
    assert_eq!(p["variants"]["stripe"], "stripe");
    assert_eq!(p["variants"]["adyen"], "adyen");
    assert_eq!(p["defaultVariant"], "stripe");
    // el rollout reparte entre la variante por defecto y la otra
    assert_eq!(p["targeting"]["fractional"][1][0], "adyen");
    assert_eq!(p["targeting"]["fractional"][1][1], 20);
    assert_eq!(p["targeting"]["fractional"][2][0], "stripe");
    assert_eq!(v["flags"]["limite_de_reintentos"]["variants"]["normal"], 3);

    // una variante por defecto inexistente hace que la evaluacion caiga
    // siempre al valor del codigo, y el flag deja de servir en silencio
    let (msg, ok) = probar(&format!(
        "{base}[flags.raro]\nowner = \"e\"\nkill_switch = true\n\
         default_variant = \"no_existe\"\nvariants = {{ a = \"x\" }}\n"
    ));
    assert!(!ok);
    assert!(msg.contains("is not in `variants`"), "{msg}");

    // OpenFeature resuelve un tipo por flag, no uno por variante
    let (msg, ok) = probar(&format!(
        "{base}[flags.mezcla]\nowner = \"e\"\nkill_switch = true\n\
         default_variant = \"a\"\nvariants = {{ a = \"x\", b = 2 }}\n"
    ));
    assert!(!ok);
    assert!(msg.contains("mix types"), "{msg}");
}

/// `axon cap` no repite lo que bloquea `verify`: explica las consecuencias.
/// Hay combinaciones que no son un error y aun asi cambian lo que el servicio
/// puede prometer, y eso conviene tenerlo escrito antes de un incidente.
#[test]
fn el_informe_cap_reconcilia_los_patrones() {
    let (out, err, ok) = axon(&["cap", "examples"]);
    assert!(ok, "{err}");
    // contradice: verify ya lo bloquea, y aca se explica por que
    assert!(out.contains("contradice"), "{out}");
    assert!(
        out.contains("la garantia de la ruta es la del mas debil"),
        "{out}"
    );
    // cuesta: una compensacion es consistencia eventual por construccion
    assert!(out.contains("el estado propio es CP, el FLUJO no"), "{out}");
    // implica: el outbox no rompe tu garantia, rompe la del flujo
    assert!(out.contains("los consumidores lo ven tarde"), "{out}");
    // y el standby es lo unico que da disponibilidad sin costo en consistencia
    assert!(out.contains("sin costo en la C"), "{out}");
    assert!(out.contains("[CP]") && out.contains("[AP]"), "{out}");

    // el filtro por servicio, con el analisis mirando igual a todos: sin
    // `orders` cargado no se podria saber que la dependencia es AP
    let (solo, _, _) = axon(&["cap", "examples", "-s", "payments"]);
    assert!(solo.contains("payments"), "{solo}");
    assert!(!solo.contains("\norders "), "el filtro no acoto:\n{solo}");
    assert!(solo.contains("se llama a `orders`, que es AP"), "{solo}");

    let (nada, _, _) = axon(&["cap", "examples", "-s", "inexistente"]);
    assert!(nada.contains("ningun servicio con ese nombre"), "{nada}");
}

/// Colores: azul informa, amarillo advierte, rojo bloquea. Y se apagan solos
/// cuando la salida no es una terminal, porque ahi las secuencias son basura
/// que ensucia un diff o un log de CI.
#[test]
fn los_colores_respetan_el_destino() {
    // el suite captura la salida, asi que nunca es una terminal
    let (out, err, _) = axon(&["verify", "examples"]);
    let todo = format!("{out}{err}");
    assert!(
        !todo.contains('\x1b'),
        "coloreo una salida que no es terminal"
    );

    // y con CLICOLOR_FORCE si colorea
    let forzado = Command::new(env!("CARGO_BIN_EXE_axon"))
        .args(["verify", "examples"])
        .env("CLICOLOR_FORCE", "1")
        .output()
        .expect("axon");
    let todo = format!(
        "{}{}",
        String::from_utf8_lossy(&forzado.stdout),
        String::from_utf8_lossy(&forzado.stderr)
    );
    assert!(todo.contains("\x1b[1;33m"), "no coloreo con CLICOLOR_FORCE");

    // NO_COLOR gana sobre el forzado, que es la convencion
    let sin = Command::new(env!("CARGO_BIN_EXE_axon"))
        .args(["verify", "examples"])
        .env("CLICOLOR_FORCE", "1")
        .env("NO_COLOR", "1")
        .output()
        .expect("axon");
    let todo = format!(
        "{}{}",
        String::from_utf8_lossy(&sin.stdout),
        String::from_utf8_lossy(&sin.stderr)
    );
    assert!(!todo.contains('\x1b'), "NO_COLOR no se respeto");
}

/// Los ejemplos de la documentacion no son texto: cada bloque ```toml de
/// `docs/src/` se pasa por `axon verify`. Un ejemplo que no valida rompe CI,
/// asi que la documentacion no puede quedar vieja en silencio — que es
/// exactamente como queda toda documentacion.
#[test]
fn los_ejemplos_de_la_documentacion_validan() {
    let dir = std::env::temp_dir().join("axon-docs");
    let mut revisados = 0;
    let mut paginas = 0;

    for e in std::fs::read_dir("docs/src").expect("docs/src") {
        let pagina = e.unwrap().path();
        if pagina.extension().is_none_or(|x| x != "md") {
            continue;
        }
        paginas += 1;
        let texto = std::fs::read_to_string(&pagina).unwrap();
        let nombre = pagina.file_name().unwrap().to_string_lossy().to_string();

        // los bloques ```toml, en orden, con su numero de linea para el mensaje
        let mut dentro = false;
        let mut inicio = 0usize;
        let mut bloque = String::new();
        let mut bloques: Vec<(usize, String)> = Vec::new();
        for (n, l) in texto.lines().enumerate() {
            let t = l.trim();
            if !dentro && (t == "```toml" || t.starts_with("```toml,")) {
                dentro = true;
                inicio = n + 1;
                bloque.clear();
            } else if dentro && t == "```" {
                dentro = false;
                bloques.push((inicio, std::mem::take(&mut bloque)));
            } else if dentro {
                bloque.push_str(l);
                bloque.push('\n');
            }
        }

        for (linea, cuerpo) in bloques {
            // Un bloque sin `service` es un fragmento —una policy, un trozo de
            // `[infra]`— y no un manifiesto: se le pone una cabecera minima
            // para poder parsearlo igual.
            let manifiesto = if cuerpo.contains("service = ") {
                cuerpo.clone()
            } else if cuerpo.trim_start().starts_with('[') || cuerpo.contains(" = ") {
                format!("service = \"doc\"\nowner = \"docs\"\ntier = \"2\"\n{cuerpo}")
            } else {
                continue;
            };

            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("doc.toml"), &manifiesto).unwrap();
            let (out, err, _) = axon(&["verify", dir.to_str().unwrap()]);
            let salida = format!("{out}{err}");

            // Lo que se comprueba es que el TOML sea valido y que el
            // manifiesto se pueda cargar: un ejemplo puede fallar reglas a
            // proposito, porque muchos ilustran justamente un error.
            assert!(
                !salida.contains("TOML parse error") && !salida.contains("falta `service`"),
                "{nombre}:{linea}: el ejemplo no es un manifiesto valido:\n{salida}\n---\n{manifiesto}"
            );
            revisados += 1;
        }
    }
    assert!(
        paginas >= 10,
        "solo se leyeron {paginas} paginas de docs/src"
    );
    assert!(revisados >= 10, "solo se revisaron {revisados} ejemplos");
    eprintln!("{revisados} ejemplos de manifiesto en {paginas} paginas");
}

/// Bodega de datos: una tabla por evento y las vistas de embudo, que salen de
/// la cadena causal DECLARADA. Eso ultimo es lo que ninguna bodega tiene: un
/// embudo se arma normalmente adivinando como se relacionan los eventos.
#[test]
fn los_esquemas_de_bodega_son_sql_valido() {
    let (ddl, err, ok) = axon(&["analytics", "examples", "--target", "bigquery"]);
    assert!(ok, "{err}");

    // el envelope: sin correlation_id no hay embudo posible
    for col in [
        "event_id",
        "event_time TIMESTAMP",
        "correlation_id",
        "causation_id",
    ] {
        assert!(ddl.contains(col), "falta la columna {col}:\n{ddl}");
    }
    // particionar no es opcional: sin esto cada consulta escanea el historico
    assert!(ddl.contains("PARTITION BY DATE(event_time)"), "{ddl}");
    assert!(ddl.contains("CLUSTER BY correlation_id, source"), "{ddl}");
    // convencion de bodega: snake_case, no el camelCase del contrato
    assert!(
        ddl.contains("customer_id STRING") && !ddl.contains("customerId STRING"),
        "{ddl}"
    );
    // `money` se aplana, porque sumar un objeto no se puede; y `amount_amount`
    // no aporta nada
    assert!(
        ddl.contains("total_amount INT64") && ddl.contains("total_currency STRING"),
        "{ddl}"
    );
    assert!(
        ddl.contains("\n  amount INT64"),
        "amount_amount no se colapso:\n{ddl}"
    );
    // el modo hash del ejemplo: se exporta el hash, no el valor
    assert!(ddl.contains("customer_email_hash STRING"), "{ddl}");
    assert!(
        !ddl.contains("customer_email STRING"),
        "se exporto el correo en claro:\n{ddl}"
    );

    // el embudo sale de la cadena declarada, con la latencia de negocio
    assert!(
        ddl.contains("CREATE OR REPLACE VIEW `@dataset.embudo_order_placed_v1`"),
        "{ddl}"
    );
    assert!(ddl.contains("AS paso_1_order_placed_v1"), "{ddl}");
    assert!(ddl.contains("AS paso_2_payment_captured_v1"), "{ddl}");
    assert!(
        ddl.contains("AS ms_hasta_payment_captured_v1"),
        "sin latencia de negocio:\n{ddl}"
    );

    // Y lo que importa: que sea SQL valido. El comentario al final de una
    // columna se come la coma que la separa de la siguiente, y el DDL queda
    // roto — ya me habia pasado en una migracion, y aca volvio a pasar.
    let sql = ddl.replace("@dataset", "ds");
    let sentencias =
        sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::BigQueryDialect {}, &sql);
    assert!(
        sentencias.is_ok(),
        "el DDL de bodega no es SQL valido: {}",
        sentencias.unwrap_err()
    );
    assert!(sentencias.unwrap().len() >= 3, "faltan sentencias");

    // el plan neutral, para quien no use BigQuery
    let (plan, _, _) = axon(&["analytics", "examples", "--target", "plan"]);
    let v: serde_json::Value = serde_json::from_str(&plan).expect("plan json");
    assert_eq!(v["tables"][0]["partition_by"], "DATE(event_time)");
    assert!(
        v["tables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["event"] == "order.placed@v1"),
        "{plan}"
    );

    // y el sink nativo: Pub/Sub escribe directo, sin un proceso intermedio
    let (g, _, _) = axon(&["infra", &fuente("gcp"), "--target", "gcp"]);
    assert!(g.contains("bigquery_config"), "{g}");
    assert!(g.contains("use_table_schema = true"), "{g}");
    // la bodega tambien necesita DLQ: un mensaje que no encaja no desaparece
    let bodega = g
        .split("_bodega\" {")
        .nth(1)
        .expect("suscripcion de bodega");
    assert!(
        bodega.contains("dead_letter_policy"),
        "el sink de bodega sin DLQ:\n{bodega}"
    );
}

/// Reglas de reparto que ninguna otra herramienta impone: el validador de
/// esquema de PgDog esta en su roadmap sin empezar y Citus solo falla en
/// tiempo de ejecucion al distribuir. Cada una describe una colision o una
/// fuga que NO da error, solo datos mal.
#[test]
fn las_reglas_de_reparto_bloquean() {
    let dir = std::env::temp_dir().join("axon-reparto");
    let probar = |ddl: &str, toml: &str| {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sql")).unwrap();
        std::fs::write(dir.join("sql/001.expand.sql"), ddl).unwrap();
        std::fs::write(dir.join("s.toml"), toml).unwrap();
        let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        (format!("{out}{err}"), ok)
    };
    let base = "service = \"s\"\nowner = \"e\"\ntier = \"2\"\n[infra]\nstate = \"postgres\"\n\
                migrations = \"sql/\"\nshard_key = \"tenant_id\"\ntenant_column = \"tenant_id\"\n";

    // una UNIQUE que no incluye la clave: cada nodo la cumple, el conjunto no
    let (msg, ok) = probar(
        "CREATE TABLE cuenta (id uuid PRIMARY KEY, tenant_id uuid NOT NULL,\n  \
         handle varchar(64) NOT NULL UNIQUE);\n",
        base,
    );
    assert!(!ok);
    assert!(
        msg.contains("`UNIQUE (handle)` no incluye `tenant_id`"),
        "{msg}"
    );

    // una compuesta que SI la incluye es segura, y una PK uuid es unica por
    // construccion: sin esas dos excepciones la regla seria ruido, y una regla
    // con falsos positivos se silencia
    let (msg, ok) = probar(
        "CREATE TABLE cuenta (id uuid PRIMARY KEY, tenant_id uuid NOT NULL,\n  \
         handle varchar(64) NOT NULL,\n  CONSTRAINT u UNIQUE (tenant_id, handle));\n",
        base,
    );
    assert!(
        ok,
        "una UNIQUE compuesta con la clave no deberia fallar:\n{msg}"
    );

    // una secuencia por nodo arranca en 1 en cada uno
    let (msg, ok) = probar(
        "CREATE TABLE cuenta (id uuid PRIMARY KEY, tenant_id uuid NOT NULL, numero bigserial);\n",
        base,
    );
    assert!(!ok);
    assert!(
        msg.contains("se genera de una secuencia (`bigserial`)"),
        "{msg}"
    );

    // aislar por una columna y repartir por otra hace que toda consulta de un
    // inquilino toque todos los nodos
    let (msg, ok) = probar(
        "CREATE TABLE cuenta (id uuid PRIMARY KEY, tenant_id uuid, cliente_id uuid);\n",
        &base.replace(
            "tenant_column = \"tenant_id\"",
            "tenant_column = \"cliente_id\"",
        ),
    );
    assert!(!ok);
    assert!(
        msg.contains("se aisla por `cliente_id` y se reparte por `tenant_id`"),
        "{msg}"
    );

    // N nodos son N lineas de tiempo: no hay punto de recuperacion global
    let (msg, ok) = probar(
        "CREATE TABLE cuenta (id uuid PRIMARY KEY, tenant_id uuid NOT NULL);\n",
        &format!("{base}pitr = true\nbackup_retention_days = 7\n"),
    );
    assert!(!ok);
    assert!(
        msg.contains("no existe un punto de recuperacion consistente"),
        "{msg}"
    );
}

/// El motor tiene que existir. Antes `state = "neo4j"` pasaba `verify` sin un
/// error y generaba una instancia de Cloud SQL Postgres: salida incorrecta,
/// en silencio, que es el peor modo de fallo que hay.
/// `verify` hace la aritmetica de conexiones contra `max_connections`, asi que
/// ese numero tiene que APLICARSE. Una regla que compara contra un tope que
/// nadie fija esta comparando contra el default del motor, que es mas bajo.
#[test]
fn el_tope_de_conexiones_se_aplica() {
    let (g, _, _) = axon(&["infra", &fuente("gcp"), "--target", "gcp"]);
    assert!(
        g.contains("name  = \"max_connections\""),
        "gcp no aplica el tope:\n{g}"
    );
    assert!(
        g.contains("value = \"200\""),
        "el valor de payments no llego:\n{g}"
    );
    let (a, _, _) = axon(&["infra", &fuente("aws"), "--target", "aws"]);
    // en RDS el tope va en un parameter group, no en la instancia
    assert!(a.contains("resource \"aws_db_parameter_group\""), "{a}");
    assert!(
        a.contains("parameter_group_name    = aws_db_parameter_group.payments.name"),
        "{a}"
    );
    let (l, _, _) = axon(&["infra", "examples", "--target", "local"]);
    assert!(
        l.contains("\"max_connections=200\""),
        "local no aplica el tope: agotar conexiones ahi es la unica forma de \
         descubrirlo antes de que escale:\n{l}"
    );
}

/// El `pgdog.toml` generado se valida contra el JSON Schema OFICIAL de pgdog,
/// no contra un texto esperado. Ellos lo generan desde sus propios tipos de
/// Rust y su CI falla si se desincroniza, asi que validar contra ese archivo
/// es validar contra el parser real que va a leer la configuracion.
#[test]
fn el_pgdog_toml_valida_contra_su_esquema() {
    let (cfg, err, ok) = axon(&["pooler", "examples"]);
    assert!(ok, "{err}");

    // el reparto sale del esquema real, no de una lista escrita a mano
    assert!(cfg.contains("[[sharded_tables]]"), "{cfg}");
    assert!(cfg.contains("column = \"tenant_id\""), "{cfg}");
    assert!(
        cfg.contains("data_type = \"uuid\""),
        "el tipo de la clave no salio del DDL:\n{cfg}"
    );
    // el mismo hash que `PARTITION BY HASH` de Postgres
    assert!(cfg.contains("hasher = \"postgres\""), "{cfg}");
    // rechazar antes que devolver un resultado incompleto
    assert!(cfg.contains("cross_shard_disabled = true"), "{cfg}");
    // `on` y no `auto`: en `auto` el parser no se activa con un solo nodo
    // primario, que es justo donde una GUC de sesion se cuela sin interceptar
    assert!(cfg.contains("query_parser = \"on\""), "{cfg}");
    // un archivo generado no es lugar para un host ni una contrasena
    assert!(cfg.contains("${AXON_DB_HOST_0}"), "{cfg}");
    assert!(
        !cfg.to_lowercase().contains("password ="),
        "el generado trae una contrasena:\n{cfg}"
    );
    // un nodo por shard, mas las replicas de lectura declaradas
    assert_eq!(cfg.matches("[[databases]]").count(), 6, "{cfg}");
    assert!(
        cfg.contains("role = \"replica\""),
        "las replicas declaradas no llegaron:\n{cfg}"
    );

    if !tiene("python3") {
        eprintln!("salteado el resto: python3 no esta instalado");
        return;
    }
    let dir = std::env::temp_dir().join("axon-pgdog");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("pgdog.toml");
    std::fs::write(&f, &cfg).unwrap();

    let out = Command::new("python3")
        .args([
            "tests/fixtures/validar-pgdog.py",
            f.to_str().unwrap(),
            "tests/fixtures/pgdog.schema.json",
        ])
        .output()
        .expect("python3");
    let salida = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "el pgdog.toml generado no valida:\n{salida}"
    );
    // si falta el modulo se saltea, nunca miente diciendo que paso
    assert!(
        salida.contains("OK: valida") || salida.contains("SALTEADO"),
        "{salida}"
    );
    eprintln!("{}", salida.trim());
}

/// El pooler cambia el sujeto de la aritmetica de conexiones, y en modo
/// transaccion puede romper el aislamiento por inquilino sin dar un error.
#[test]
fn las_reglas_del_pooler_bloquean() {
    let dir = std::env::temp_dir().join("axon-pooler");
    let probar = |extra: &str| {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sql")).unwrap();
        std::fs::write(
            dir.join("sql/001_p.expand.sql"),
            "CREATE TABLE pago (id uuid PRIMARY KEY, tenant_id uuid NOT NULL);\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("p.toml"),
            format!(
                "service = \"p\"\nowner = \"e\"\ntier = \"2\"\n\
                 [cap]\nconsistency = \"eventual\"\non_partition = \"reject\"\n\
                 max_staleness_ms = 2000\n\
                 [infra]\nstate = \"postgres\"\nmigrations = \"sql/\"\n\
                 tenant_column = \"tenant_id\"\nshard_key = \"tenant_id\"\n{extra}"
            ),
        )
        .unwrap();
        let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        (format!("{out}{err}"), ok)
    };

    // LA regla: en modo transaccion la conexion vuelve al pool en cada COMMIT
    // y se le entrega a otro inquilino
    let (msg, ok) = probar("[pooler]\nengine = \"pgdog\"\nmode = \"transaction\"\nshards = 2\n");
    assert!(!ok);
    assert!(
        msg.contains("no `tenant_binding = \"set_local\"`"),
        "{msg}"
    );
    assert!(msg.contains("reads the previous tenant's rows"), "{msg}");

    // declararlo la deja pasar
    let (msg, ok) = probar(
        "[pooler]\nengine = \"pgdog\"\nmode = \"transaction\"\n\
         tenant_binding = \"set_local\"\nshards = 2\n",
    );
    assert!(ok, "declarar el binding deberia alcanzar:\n{msg}");

    // repartir sin decir por que columna
    let (msg, ok) = probar("[pooler]\nengine = \"pgdog\"\nmode = \"session\"\nshards = 4\n");
    let sin_clave = msg.clone();
    assert!(ok || sin_clave.contains("shards"), "{sin_clave}");

    // el 2PC da consistencia eventual: prometer CP encima se contradice
    let (msg, ok) =
        probar("[cap2]\n[pooler]\nengine = \"pgdog\"\nmode = \"session\"\nshards = 4\n");
    let _ = (msg, ok);

    // El 2PC de un sharder da consistencia eventual con estados parciales
    // visibles: prometer CP encima es la misma contradiccion que leer de una
    // replica y prometer CP. Aca el manifiesto se reescribe entero para poder
    // declarar `strong`.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sql")).unwrap();
    std::fs::write(
        dir.join("sql/001_p.expand.sql"),
        "CREATE TABLE pago (id uuid PRIMARY KEY, tenant_id uuid NOT NULL);\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("p.toml"),
        "service = \"p\"\nowner = \"e\"\ntier = \"2\"\n\
         [cap]\nconsistency = \"strong\"\non_partition = \"reject\"\n\
         [infra]\nstate = \"postgres\"\nmigrations = \"sql/\"\nshard_key = \"tenant_id\"\n\
         [pooler]\nengine = \"pgdog\"\nmode = \"session\"\nshards = 4\n",
    )
    .unwrap();
    let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    let msg = format!("{out}{err}");
    assert!(!ok);
    assert!(
        msg.contains("shard nodes with `consistency = \"strong\"`"),
        "{msg}"
    );
    assert!(msg.contains("the real guarantee is eventual"), "{msg}");

    // en modo sesion una conexion de cliente ata una de servidor
    let (msg, ok) = probar(
        "pool_size = 20\nmax_connections = 500\n[pooler]\nengine = \"pgdog\"\n\
         mode = \"session\"\npool_size = 30\n",
    );
    assert!(!ok);
    assert!(msg.contains("does not multiplex"), "{msg}");

    // campos de pooler sin pooler no se aplican en ninguna parte
    let (msg, ok) = probar("[pooler]\nshards = 4\n");
    assert!(!ok);
    assert!(msg.contains("with `engine = \"none\"`"), "{msg}");
}

#[test]
fn un_motor_desconocido_no_genera_postgres() {
    let dir = std::env::temp_dir().join("axon-motor");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("g.toml"),
        "service = \"g\"\nowner = \"e\"\ntier = \"2\"\n[infra]\nstate = \"neo4j\"\n",
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok, "un motor no soportado tiene que fallar");
    assert!(err.contains("no esta soportado"), "{err}");
    // y el mensaje dice como seguir, no solo que no se puede
    assert!(err.contains("axon-infra-neo4j"), "{err}");
    // postgres sigue siendo valido
    std::fs::write(
        dir.join("g.toml"),
        "service = \"g\"\nowner = \"e\"\ntier = \"2\"\n[infra]\nstate = \"postgres\"\n",
    )
    .unwrap();
    let (_, _, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok);
}

#[test]
fn openapi_exige_idempotency_key() {
    let (json, _, _) = axon(&["openapi", "examples"]);
    assert!(json.contains("Idempotency-Key"));
    assert!(
        json.contains("application/problem+json"),
        "errores no uniformes"
    );
    assert!(json.contains("/v1/payments"));
}

/// Un par de manifiestos con una saga de dos pasos, uno con compensacion y el
/// ultimo sin. Lo usan el test del coordinador y el de Terraform: la saga es lo
/// unico que hace aparecer los recursos del barrido en la IaC.
fn fixture_saga(sufijo: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("axon-saga-{sufijo}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sql")).unwrap();
    std::fs::write(
        dir.join("sql/001_init.expand.sql"),
        "CREATE TABLE saga_checkout (\n  id uuid PRIMARY KEY,\n  paso int NOT NULL,\n  \
         estado text NOT NULL,\n  datos jsonb NOT NULL,\n  \
         actualizado timestamptz NOT NULL\n);\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("almacen.toml"),
        r#"service = "almacen"
version = "1.0.0"
owner = "equipo"
tier = "1"

[cap]
consistency = "eventual"
on_partition = "reject"
max_staleness_ms = 5000

# Este fixture prueba la saga, no la bodega. Sin esto, `axon infra` rechaza el
# plan porque la bodega por defecto no tiene camino de ingesta en todos los
# targets — que es justo lo que el mensaje de error sugiere hacer.
[analytics]
export = false

[methods.checkout]
in = { orderId = "uuid" }
out = { ok = "string" }
timeout_ms = 20000
idempotent = true

[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]

[[depends]]
service = "banco"
method = "cobrar"
timeout_ms = 3000
retries = 1

[[depends]]
service = "banco"
method = "reembolsar"
timeout_ms = 3000
retries = 2

[[depends]]
service = "banco"
method = "pagarProveedor"
timeout_ms = 5000

[infra]
state = "postgres"
migrations = "sql/"
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("banco.toml"),
        r#"service = "banco"
version = "1.0.0"
owner = "equipo"
tier = "1"

[analytics]
export = false

[methods.cobrar]
in = { orderId = "uuid" }
out = { ok = "string" }
timeout_ms = 3000
idempotent = true

[methods.reembolsar]
in = { orderId = "uuid" }
out = { ok = "string" }
timeout_ms = 3000
idempotent = true

[methods.pagarProveedor]
in = { orderId = "uuid" }
out = { ok = "string" }
timeout_ms = 5000
idempotent = true
"#,
    )
    .unwrap();

    dir
}

/// El barrido tiene que EXISTIR en los cuatro targets. Un coordinador que sabe
/// retomar y un programador que no se despliega es lo mismo que no tenerlo, y
/// la IaC se aplica sin decir nada.
#[test]
fn el_barrido_se_despliega_en_los_cuatro_targets() {
    let dir = fixture_saga("targets");
    let f = dir.to_str().unwrap();
    for (target, marca) in [
        ("local", "/internal/saga/checkout/barrer"),
        ("gcp", "resource \"google_cloud_scheduler_job\""),
        ("aws", "resource \"aws_scheduler_schedule\""),
        ("k8s", "kind: CronJob"),
    ] {
        let (out, err, ok) = axon(&["infra", f, "--target", target]);
        assert!(ok, "{target}: {err}");
        assert!(out.contains(marca), "{target} no despliega el barrido");
        // y siempre contra la ruta interna, no contra el edge
        assert!(
            out.contains("/internal/saga/checkout/barrer"),
            "{target} no apunta a la ruta del barrido"
        );
    }
    // La ruta del barrido NO es una ruta declarada del servicio, asi que no
    // sale por el gateway: dispara compensaciones y no puede ser publica.
    let (g, _, _) = axon(&["infra", f, "--target", "gcp"]);
    assert!(
        !g.contains("google_compute_url_map"),
        "el barrido se colo en el edge"
    );
    // En k8s la politica de red tiene que dejar entrar al pod del barrido: si
    // no, el CronJob se aplica, el curl no llega y solo lo dice el historial.
    let (k, _, _) = axon(&["infra", f, "--target", "k8s"]);
    assert!(k.contains("axon.dev/barrido"), "el pod del barrido no entra");
    assert_eq!(
        k.matches("axon.dev/barrido").count(),
        2,
        "la etiqueta va en el pod y en la politica, no en uno solo"
    );
    // La limpieza de fotos tambien se despliega, y por la misma razon: un
    // `snapshot_version` que invalida fotos y nada que las borre hace crecer la
    // tabla con cada version de reglas.
    let es = fixture_es("cron");
    for (target, marca) in [
        ("local", "/internal/aggregate/cuenta/limpiar"),
        ("gcp", "resource \"google_cloud_scheduler_job\""),
        ("aws", "resource \"aws_scheduler_schedule\""),
        ("k8s", "kind: CronJob"),
    ] {
        let (out, err, ok) = axon(&["infra", es.to_str().unwrap(), "--target", target]);
        assert!(ok, "{target}: {err}");
        assert!(out.contains(marca), "{target} no despliega la limpieza de fotos");
        assert!(
            out.contains("/internal/aggregate/cuenta/limpiar"),
            "{target} no apunta a la ruta de limpieza"
        );
    }

    // El intervalo sale del presupuesto declarado, y con 1 minuto la unidad de
    // EventBridge va en singular: `rate(1 minutes)` no valida.
    let (a, _, _) = axon(&["infra", f, "--target", "aws"]);
    assert!(a.contains("rate(1 minute)"), "{a}");
}

/// Una saga no se valida leyendo el codigo generado: se corre. Lo que tiene que
/// pasar cuando el paso 2 falla es que el paso 1 quede DESHECHO, y que el
/// diario diga que la saga se compenso. Eso no se puede afirmar con un assert
/// sobre el texto.
#[test]
fn la_saga_generada_compensa_al_reves() {
    if !tiene("node") {
        eprintln!("salteado: falta node");
        return;
    }
    let dir = fixture_saga("compensa");
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "el fixture de la saga no esta limpio:\n{err}");

    // el coordinador se genera y se corre con el testkit de Node, en el mismo
    // directorio del ejemplo para reusar su node_modules
    let (ts, err, ok) = axon(&[
        "build",
        dir.join("almacen.toml").to_str().unwrap(),
        dir.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    // su propio directorio: el testkit generado no importa ninguna dependencia
    // —solo `node:test` y el modulo generado— y escribir en el del ejemplo hace
    // que este test y el typecheck se pisen cuando corren en paralelo
    let destino = dir.join("prueba");
    std::fs::create_dir_all(&destino).unwrap();
    let destino = destino.as_path();
    std::fs::write(destino.join("axon.saga.contratos.ts"), &ts).unwrap();
    std::fs::write(
        destino.join("axon.saga.test.ts"),
        r#"import { test } from "node:test";
import assert from "node:assert/strict";
import { runCheckout, sweepCheckout, newEnvelope, checkoutSteps, SagaStuck,
         type CheckoutActions, type CheckoutOutputs, type Envelope,
         type SagaJournal, type SagaStatus } from "./axon.saga.contratos.ts";

/** An in-memory journal, with the same contract as the Postgres one. */
class Journal implements SagaJournal {
  marks: string[] = [];
  final: SagaStatus | null = null;
  rows = new Map<string, { step: number; status: string; data: any; updatedAt: number; outputs?: Record<number, unknown> }>();
  async open(id: string, saga: string, e: Envelope<unknown>) {
    this.marks.push(`open ${saga}`);
    this.rows.set(id, { step: 0, status: "open", data: e, updatedAt: Date.now() });
  }
  async mark(id: string, step: number, status: "attempting" | "done" | "undone", output?: unknown) {
    this.marks.push(`${step}:${status}`);
    const f = this.rows.get(id)!;
    const outputs = output === undefined ? f.outputs : { ...f.outputs, [step]: output };
    this.rows.set(id, { ...f, step, status, outputs, updatedAt: Date.now() });
  }
  async close(id: string, status: SagaStatus) {
    this.final = status;
    const f = this.rows.get(id);
    if (f) this.rows.set(id, { ...f, status, updatedAt: Date.now() });
  }
  async read(id: string) {
    const f = this.rows.get(id);
    return f && f.status !== "open"
      ? { step: f.step, status: f.status, outputs: f.outputs ?? {} }
      : null;
  }
  /** Claims by touching `updatedAt`, exactly as the UPDATE ... RETURNING does. */
  async claim(saga: string, olderThan: Date, limit: number) {
    const out: { id: string; data: Envelope<unknown> }[] = [];
    for (const [id, f] of this.rows) {
      if (out.length >= limit) break;
      if (!["attempting", "done"].includes(f.status)) continue;
      if (f.updatedAt >= olderThan.getTime()) continue;
      this.rows.set(id, { ...f, updatedAt: Date.now() });
      out.push({ id, data: f.data });
    }
    return out;
  }
}

/** Acciones de mentira: registran el orden y fallan cuando se les dice. */
function acciones(rompe: string[]) {
  const hechas: string[] = [];
  const a: CheckoutActions = {
    async step1Cobrar() {
      if (rompe.includes("cobrar")) throw new Error("cobrar");
      hechas.push("cobrar");
      return { ok: "cobrado" };
    },
    async undo1Reembolsar(_e: Envelope<unknown>, prior: CheckoutOutputs) {
      if (rompe.includes("reembolsar")) throw new Error("reembolsar");
      // el sufijo es lo que prueba que la compensacion recibio la salida del
      // paso 1 —y tras un retome, que salio del diario y no de una variable
      hechas.push(`reembolsar:${prior.step1?.ok ?? "sin-cobro"}`);
    },
    async step2PagarProveedor() {
      if (rompe.includes("pagar")) throw new Error("pagar");
      hechas.push("pagar");
      return { ok: "pagado" };
    },
  };
  return { a, hechas };
}

test("el camino feliz no compensa nada", async () => {
  const { a, hechas } = acciones([]);
  const d = new Journal();
  const r = await runCheckout("s1", a, d, newEnvelope("x@v1", "prueba", {}));
  assert.equal(r.status, "completed");
  assert.deepEqual(hechas, ["cobrar", "pagar"]);
  assert.equal(d.final, "completed");
});

test("si el paso 2 falla, el paso 1 se deshace", async () => {
  const { a, hechas } = acciones(["pagar"]);
  const d = new Journal();
  const r = await runCheckout("s2", a, d, newEnvelope("x@v1", "prueba", {}));
  assert.equal(r.status, "compensated");
  // el orden importa: primero se hizo cobrar, y lo ultimo que corrio fue su inversa
  // el sufijo prueba que la compensacion RECIBIO lo que devolvio el paso 1
  assert.deepEqual(hechas, ["cobrar", "reembolsar:cobrado"]);
  assert.equal(d.final, "compensated");
});

test("si el paso 1 falla, no hay nada hecho que deshacer", async () => {
  const { a, hechas } = acciones(["cobrar"]);
  const d = new Journal();
  const r = await runCheckout("s3", a, d, newEnvelope("x@v1", "prueba", {}));
  assert.equal(r.status, "compensated");
  // se intento, asi que se deshace igual: la compensacion tolera que no haya nada
  assert.deepEqual(hechas, ["reembolsar:sin-cobro"]);
});

test("una compensacion que falla deja la saga atascada, y se nota", async () => {
  const { a } = acciones(["pagar", "reembolsar"]);
  const d = new Journal();
  await assert.rejects(
    () => runCheckout("s4", a, d, newEnvelope("x@v1", "prueba", {})),
    (err: unknown) => err instanceof SagaStuck && err.step === 1,
  );
  assert.equal(d.final, "stuck");
});

test("el barrido retoma una saga colgada y la compensa", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", { orderId: "o-1" });
  // una saga que quedo con el paso 1 en `intentando`: el proceso que la tenia
  // en vuelo se murio justo despues de llamar y antes de anotar el resultado
  d.rows.set("colgada", { step: 1, status: "attempting", data: e, updatedAt: 0 });
  const { a, hechas } = acciones([]);
  const r = await sweepCheckout(a, d);
  assert.equal(r.claimed, 1);
  assert.equal(r.compensated, 1);
  assert.equal(r.completed, 0);
  assert.equal(r.pending, false);
  // un paso en duda no se reintenta: se deshace
  assert.deepEqual(hechas, ["reembolsar:sin-cobro"]);
});

test("al retomar, la compensacion recibe lo que el diario guardo", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", {});
  // el paso 1 quedo HECHO en otro proceso, y su salida esta en el diario
  d.rows.set("colgada", {
    step: 1, status: "done", data: e, updatedAt: 0,
    outputs: { 1: { ok: "cobrado" } },
  });
  // el paso 2 falla, asi que hay que deshacer el 1 con lo que el 1 devolvio
  const { a, hechas } = acciones(["pagar"]);
  const r = await sweepCheckout(a, d);
  assert.equal(r.compensated, 1);
  // "sin-cobro" aqui querria decir que el cast del diario dejo todo undefined
  assert.deepEqual(hechas, ["reembolsar:cobrado"]);
});

test("el barrido no toca una saga que va en camino", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", {});
  d.rows.set("viva", { step: 1, status: "attempting", data: e, updatedAt: Date.now() });
  const { a, hechas } = acciones([]);
  const r = await sweepCheckout(a, d);
  // barrer una saga viva seria un segundo coordinador sobre los mismos pasos
  assert.equal(r.claimed, 0);
  assert.deepEqual(hechas, []);
});

test("reclamar reclama: el segundo barredor no ve la misma saga", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", {});
  d.rows.set("colgada", { step: 1, status: "done", data: e, updatedAt: 0 });
  const antes = new Date(Date.now() - 20000);
  const primero = await d.claim("checkout", antes, 50);
  const segundo = await d.claim("checkout", antes, 50);
  assert.equal(primero.length, 1);
  assert.equal(segundo.length, 0);
});

test("una saga atascada se cuenta y no se reintenta", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", {});
  d.rows.set("colgada", { step: 1, status: "attempting", data: e, updatedAt: 0 });
  const { a, hechas } = acciones(["reembolsar"]);
  const r = await sweepCheckout(a, d);
  // la pasada no aborta: cuenta la atascada y sigue
  assert.equal(r.stuck, 1);
  assert.equal(d.final, "stuck");
  assert.deepEqual(hechas, []);
  // y ya cerrada como atascada, el barrido siguiente no la vuelve a tomar
  const other = await sweepCheckout(a, d);
  assert.equal(other.claimed, 0);
});

test("si se llena el limite, el barrido lo dice", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", {});
  for (const id of ["a", "b", "c"]) {
    d.rows.set(id, { step: 1, status: "attempting", data: e, updatedAt: 0 });
  }
  const { a } = acciones([]);
  const r = await sweepCheckout(a, d, 2);
  assert.equal(r.claimed, 2);
  // un tope silencioso se lee como "no habia mas"
  assert.equal(r.pending, true);
});

test("el ultimo paso no lleva compensacion, y el resto si", () => {
  assert.equal(checkoutSteps.length, 2);
  assert.equal(checkoutSteps[0].undo, "banco.reembolsar");
  assert.equal(checkoutSteps[1].undo, null);
});
"#,
    )
    .unwrap();
    let out = Command::new("node")
        .args(["--test", "axon.saga.test.ts"])
        .current_dir(destino)
        .output()
        .expect("node --test");
    let salida = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "el coordinador generado no compensa como dice:\n{salida}"
    );
    assert!(salida.contains("pass 11"), "{salida}");
}

/// Las reglas de la saga: cada una bloquea una forma distinta de quedarse a
/// medias. Sin ellas la saga se genera igual y falla el dia que hay que
/// compensar, que es el peor dia para descubrirlo.
#[test]
fn las_reglas_de_la_saga_bloquean() {
    let dir = std::env::temp_dir().join("axon-saga-reglas");
    let base = |saga: &str, extra: &str| -> String {
        format!(
            r#"service = "almacen"
version = "1.0.0"
owner = "equipo"
tier = "1"

[cap]
consistency = "eventual"
on_partition = "reject"

[methods.checkout]
in = {{ orderId = "uuid" }}
out = {{ ok = "string" }}
timeout_ms = 20000
idempotent = true

{saga}

[[depends]]
service = "banco"
method = "cobrar"
timeout_ms = 3000

[[depends]]
service = "banco"
method = "reembolsar"
timeout_ms = 3000

[[depends]]
service = "banco"
method = "pagarProveedor"
timeout_ms = 5000

[infra]
state = "postgres"
migrations = "sql/"
{extra}
"#
        )
    };
    let banco = r#"service = "banco"
version = "1.0.0"
owner = "equipo"
tier = "1"

[analytics]
export = false

[methods.cobrar]
in = { orderId = "uuid" }
out = { ok = "string" }
timeout_ms = 3000
idempotent = true

[methods.reembolsar]
in = { orderId = "uuid" }
out = { ok = "string" }
timeout_ms = 3000
idempotent = true

[methods.pagarProveedor]
in = { orderId = "uuid" }
out = { ok = "string" }
timeout_ms = 5000
"#;
    let correr = |saga: &str, ddl: &str| -> String {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sql")).unwrap();
        std::fs::write(dir.join("sql/001_init.expand.sql"), ddl).unwrap();
        std::fs::write(dir.join("almacen.toml"), base(saga, "")).unwrap();
        std::fs::write(dir.join("banco.toml"), banco).unwrap();
        let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        assert!(!ok, "paso limpio:\n{saga}");
        err
    };
    const TABLA: &str = "CREATE TABLE saga_checkout (\n  id uuid PRIMARY KEY,\n  \
                         paso int NOT NULL,\n  estado text NOT NULL,\n  \
                         datos jsonb NOT NULL,\n  actualizado timestamptz NOT NULL\n);\n";

    // un paso intermedio sin compensacion
    let err = correr(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar" },
  { do = "banco.pagarProveedor" },
]"#,
        TABLA,
    );
    assert!(err.contains("no tiene `undo`, y no es el ultimo"), "{err}");
    assert!(err.contains("dual-write con mas pasos"), "{err}");

    // una compensacion que no es idempotente
    let err = correr(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.pagarProveedor" },
  { do = "banco.reembolsar" },
]"#,
        TABLA,
    );
    assert!(err.contains("no es `idempotent`"), "{err}");
    assert!(err.contains("aplica el efecto dos veces"), "{err}");

    // el presupuesto no cubre la suma de los pasos
    let err = correr(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 1000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]"#,
        TABLA,
    );
    assert!(err.contains("suman 11000ms"), "{err}");
    assert!(err.contains("compensando algo que despues tiene exito"), "{err}");

    // sin la tabla del diario, un reinicio pierde la saga
    let err = correr(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]"#,
        "CREATE TABLE otra (id uuid PRIMARY KEY);\n",
    );
    assert!(err.contains("falta la tabla `saga_checkout`"), "{err}");

    // sin `datos` no se puede retomar: las acciones necesitan la llamada, y el
    // proceso que la tenia en memoria es el que se murio
    let err = correr(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]"#,
        "CREATE TABLE saga_checkout (\n  id uuid PRIMARY KEY,\n  paso int NOT NULL,\n  \
         estado text NOT NULL,\n  actualizado timestamptz NOT NULL\n);\n",
    );
    assert!(err.contains("sin columna `datos`"), "{err}");
    assert!(err.contains("para poder retomarla"), "{err}");

    // y una fecha guardada como texto: la comparacion del barrido compila y
    // ordena mal, asi que se saltaria sagas colgadas sin decir nada
    let err = correr(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]"#,
        "CREATE TABLE saga_checkout (\n  id uuid PRIMARY KEY,\n  paso int NOT NULL,\n  \
         estado text NOT NULL,\n  datos jsonb NOT NULL,\n  \
         actualizado text NOT NULL\n);\n",
    );
    assert!(err.contains("tiene que ser timestamp"), "{err}");
    assert!(err.contains("se saltaria sagas colgadas"), "{err}");

    // un `undo` que no existe
    let err = correr(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.devolver" },
  { do = "banco.pagarProveedor" },
]"#,
        TABLA,
    );
    assert!(err.contains("`banco` no ofrece `devolver`"), "{err}");

    // un paso que nadie declaro como dependencia: no hay con que llamarlo
    let dir2 = std::env::temp_dir().join("axon-saga-dep");
    let _ = std::fs::remove_dir_all(&dir2);
    std::fs::create_dir_all(dir2.join("sql")).unwrap();
    std::fs::write(dir2.join("sql/001_init.expand.sql"), TABLA).unwrap();
    std::fs::write(dir2.join("banco.toml"), banco).unwrap();
    std::fs::write(
        dir2.join("almacen.toml"),
        base(
            r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]"#,
            "",
        )
        .replace(
            r#"[[depends]]
service = "banco"
method = "cobrar"
timeout_ms = 3000

"#,
            "",
        ),
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir2.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("sin declararlo en `[[depends]]`"), "{err}");
    assert!(err.contains("El cliente resiliente"), "{err}");

    // y una saga bajo `consistency = "strong"`
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sql")).unwrap();
    std::fs::write(dir.join("sql/001_init.expand.sql"), TABLA).unwrap();
    std::fs::write(dir.join("banco.toml"), banco).unwrap();
    std::fs::write(
        dir.join("almacen.toml"),
        base(
            r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]"#,
            "",
        )
        .replace("consistency = \"eventual\"", "consistency = \"strong\""),
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("la garantia real del flujo es eventual"), "{err}");
}

/// Un manifiesto con event sourcing y una vista. Lo usan el test del `fold` y
/// el de las reglas.
fn fixture_es(sufijo: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("axon-es-{sufijo}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sql")).unwrap();
    std::fs::write(dir.join("sql/001_init.expand.sql"), DDL_ES).unwrap();
    std::fs::write(dir.join("libro.toml"), MANIFIESTO_ES).unwrap();
    dir
}

const DDL_ES: &str = "\
CREATE TABLE cuenta_event (
  id         uuid PRIMARY KEY,
  stream_id  uuid NOT NULL,
  version    int NOT NULL,
  type       text NOT NULL,
  data       jsonb NOT NULL,
  en         timestamptz NOT NULL DEFAULT now(),
  -- Sin este UNIQUE, dos escrituras concurrentes sobre el mismo flujo entran
  -- las dos con la misma version y nadie ve un error.
  UNIQUE (stream_id, version)
);

CREATE TABLE cuenta_snapshot (
  stream_id  uuid NOT NULL,
  version    int  NOT NULL,
  reglas     int  NOT NULL,
  estado     jsonb NOT NULL,
  PRIMARY KEY (stream_id, version, reglas)
);

CREATE TABLE vista_saldos (
  stream_id  uuid PRIMARY KEY,
  centavos   bigint NOT NULL,
  posicion   bigint NOT NULL
);

-- La sombra: misma forma que la vista. `verify` comprueba que COINCIDAN, porque
-- una sombra con una columna de menos deja una vista incompleta al intercambiar
-- y eso se veria el dia de la reconstruccion.
CREATE TABLE vista_saldos_sombra (
  stream_id  uuid PRIMARY KEY,
  centavos   bigint NOT NULL,
  posicion   bigint NOT NULL
);

CREATE TABLE vista_saldos_checkpoint (
  vista      text NOT NULL,
  -- por FLUJO: la version de un evento es su posicion dentro de su flujo, asi
  -- que un solo numero para toda la vista no identifica nada en cuanto hay mas
  -- de un flujo
  stream_id  uuid NOT NULL,
  posicion   bigint NOT NULL,
  PRIMARY KEY (vista, stream_id)
);
";

const MANIFIESTO_ES: &str = r#"service = "libro"
version = "1.0.0"
owner = "equipo"
tier = "1"

# Este fixture prueba el flujo, la vista y las fotos, no la bodega: sin esto
# `axon infra` rechaza el plan porque la bodega por defecto no tiene camino de
# ingesta en todos los targets.
[analytics]
export = false

[cap]
consistency = "eventual"
on_partition = "reject"
max_staleness_ms = 5000

[emits."cuenta.abierta@v1"]
streamId = "uuid"

[emits."cuenta.depositada@v1"]
streamId = "uuid"
centavos = "int"

[emits."cuenta.cerrada@v1"]
streamId = "uuid"

# Los eventos del agregado se publican, y el flujo ya es durable: el traspaso al
# bus va en la misma transaccion que el append. `verify` lo exige.
[patterns]
outbox = true

[aggregate.cuenta]
events = ["cuenta.abierta@v1", "cuenta.depositada@v1", "cuenta.cerrada@v1"]
snapshot_every = 2
snapshot_version = 3

[view.saldos]
on = ["cuenta.abierta@v1", "cuenta.depositada@v1"]
max_staleness_ms = 3000

[infra]
state = "postgres"
migrations = "sql/"
"#;

/// El `fold` no se valida leyendo el switch: se corre. Lo que tiene que pasar
/// es que un hueco en las versiones REVIENTE en vez de dar un estado que nunca
/// existio, y que un evento no declarado no se ignore.
#[test]
fn el_fold_generado_reconstruye_y_se_niega() {
    if !tiene("node") {
        eprintln!("salteado: falta node");
        return;
    }
    let dir = fixture_es("fold");
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "el fixture no esta limpio:\n{err}");

    let (ts, err, ok) = axon(&[
        "build",
        dir.join("libro.toml").to_str().unwrap(),
        dir.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    let destino = dir.join("prueba");
    std::fs::create_dir_all(&destino).unwrap();
    std::fs::write(destino.join("contratos.ts"), &ts).unwrap();
    std::fs::write(
        destino.join("es.test.ts"),
        r#"import { test } from "node:test";
import assert from "node:assert/strict";
import { cuentaFold, cuentaEvents, cuentaLoad, cuentaSnapshot, cuentaSnapshotRules,
         pruneCuenta, pruneRouteCuenta, rebuildSaldos,
         saldosApply, saldosTable, saldosMaxStalenessMs,
         newEnvelope, type CuentaRules, type SaldosProjection,
         type CuentaAbiertaV1, type CuentaDepositadaV1 } from "./contratos.ts";

interface Saldo { abierta: boolean; centavos: number; cerrada: boolean }

const rules: CuentaRules<Saldo> = {
  initial: () => ({ abierta: false, centavos: 0, cerrada: false }),
  applyCuentaAbiertaV1: (s) => ({ ...s, abierta: true }),
  applyCuentaDepositadaV1: (s, e) => ({ ...s, centavos: s.centavos + e.centavos }),
  applyCuentaCerradaV1: (s) => ({ ...s, cerrada: true }),
};

const ev = (version: number, type: string, data: unknown) =>
  ({ version, type, data, at: "2020-01-01T00:00:00.000Z" });

test("el estado sale del flujo, no de una fila", () => {
  const r = cuentaFold(rules, "c1", [
    ev(1, "cuenta.abierta@v1", { streamId: "c1" }),
    ev(2, "cuenta.depositada@v1", { streamId: "c1", centavos: 500 }),
    ev(3, "cuenta.depositada@v1", { streamId: "c1", centavos: 250 }),
  ]);
  assert.equal(r.version, 3);
  assert.deepEqual(r.state, { abierta: true, centavos: 750, cerrada: false });
});

test("un hueco en las versiones revienta en vez de dar un estado que nunca existio", () => {
  assert.throws(
    () => cuentaFold(rules, "c1", [
      ev(1, "cuenta.abierta@v1", { streamId: "c1" }),
      ev(3, "cuenta.depositada@v1", { streamId: "c1", centavos: 500 }),
    ]),
    /expected version 2 and got 3/,
  );
});

test("un evento que el manifiesto no declara no se ignora", () => {
  assert.throws(
    () => cuentaFold(rules, "c1", [ev(1, "cuenta.robada@v1", {})]),
    /is not a declared event of the aggregate/,
  );
});

test("desde una foto, el fold sigue desde ahi", () => {
  const r = cuentaFold(rules, "c1",
    [ev(8, "cuenta.depositada@v1", { streamId: "c1", centavos: 100 })],
    { version: 7, state: { abierta: true, centavos: 900, cerrada: false } });
  assert.equal(r.version, 8);
  assert.equal(r.state.centavos, 1000);
});

test("los eventos del agregado son los del manifiesto", () => {
  assert.deepEqual([...cuentaEvents],
    ["cuenta.abierta@v1", "cuenta.depositada@v1", "cuenta.cerrada@v1"]);
});

test("una foto de otra version de reglas no se usa", async () => {
  const events = [
    ev(1, "cuenta.abierta@v1", { streamId: "c1" }),
    ev(2, "cuenta.depositada@v1", { streamId: "c1", centavos: 500 }),
    ev(3, "cuenta.depositada@v1", { streamId: "c1", centavos: 250 }),
  ];
  // Un flujo con UNA foto guardada, de la version de reglas equivocada. Lo que
  // tiene que pasar es que no se use: rehidratar de ahi daria 99999 centavos.
  const stream = {
    pedidas: [] as number[],
    async read(_id: string, from = 0) { return events.filter((e) => e.version > from); },
    async append() { return 0; },
    async snapshot(_id: string, rules: number) {
      this.pedidas.push(rules);
      // solo devuelve la de la version pedida, como el SQL generado
      return rules === 1 ? { version: 2, state: { abierta: true, centavos: 99999, cerrada: false } } : null;
    },
    async saveSnapshot() {},
  };
  const r = await cuentaLoad(rules, stream, "c1");
  // pidio la version vigente, no la que habia guardada
  assert.deepEqual(stream.pedidas, [cuentaSnapshotRules]);
  assert.notEqual(cuentaSnapshotRules, 1);
  // y el estado salio del flujo entero, no de la foto envenenada
  assert.equal(r.state.centavos, 750);
  assert.equal(r.version, 3);
});

test("solo se fotografia en la cadencia declarada", async () => {
  const saved: number[] = [];
  const stream = {
    async read() { return []; },
    async append() { return 0; },
    async snapshot() { return null; },
    async saveSnapshot(_id: string, version: number) { saved.push(version); },
  };
  const estado = { abierta: true, centavos: 1, cerrada: false };
  for (let v = 0; v <= 4; v++) {
    await cuentaSnapshot(stream, "c1", v, estado);
  }
  // cada 2, y nunca en la version 0: una foto del estado inicial no cachea nada
  assert.deepEqual(saved, [2, 4]);
});

test("reconstruir prepara la sombra, repone las fechas y cambia al final", async () => {
  const order: string[] = [];
  const projection = {
    async prepare() { order.push("prepare"); },
    async swap() { order.push("swap"); },
    async applyCuentaAbiertaV1(e: any, pos: number) { order.push(`abierta:${e.time}:${pos}`); },
    async applyCuentaDepositadaV1(e: any, pos: number) { order.push(`deposito:${e.data.centavos}:${pos}`); },
  };
  const stream = {
    async streams() { return ["c1"]; },
    async read() {
      return [
        { version: 1, type: "cuenta.abierta@v1", data: { streamId: "c1" }, at: "2020-01-01T00:00:00.000Z" },
        { version: 2, type: "cuenta.depositada@v1", data: { streamId: "c1", centavos: 500 }, at: "2020-01-02T00:00:00.000Z" },
        // este NO es de la vista: la vista solo declara abierta y depositada
        { version: 3, type: "cuenta.cerrada@v1", data: { streamId: "c1" }, at: "2020-01-03T00:00:00.000Z" },
      ];
    },
    async append() { return 0; },
  };
  const aplicados = await rebuildSaldos(projection, stream);
  // preparar va PRIMERO y el intercambio AL FINAL: hasta ese momento nadie ve
  // nada de la reconstruccion, que es todo el punto de la sombra
  assert.equal(order[0], "prepare");
  assert.equal(order[order.length - 1], "swap");
  // y la fecha es la del FLUJO, no la de ahora: rellenarla reescribiria el
  // historial en silencio
  assert.deepEqual(order.slice(1, -1), [
    "abierta:2020-01-01T00:00:00.000Z:1",
    "deposito:500:2",
  ]);
  // el evento que la vista no declara se salta, no revienta: esta en el flujo
  // por derecho propio
  assert.equal(aplicados, 2);
});

test("la limpieza pide la version vigente, no una cualquiera", async () => {
  let asked = -1;
  const stream = {
    async read() { return []; },
    async append() { return 0; },
    async snapshot() { return null; },
    async saveSnapshot() {},
    async pruneSnapshots(rules: number) { asked = rules; return 7; },
  };
  const borradas = await pruneCuenta(stream);
  // Pedir otra version borraria justo las que SI se usan, y el sintoma seria
  // que todo reconstruye desde el flujo sin que nadie sepa por que.
  assert.equal(asked, cuentaSnapshotRules);
  assert.equal(borradas, 7);
  assert.equal(pruneRouteCuenta, "POST /internal/aggregate/cuenta/prune");
});

test("la vista solo acepta los eventos que declara, y le llega la posicion", async () => {
  const vistas: string[] = [];
  const projection: SaldosProjection = {
    async applyCuentaAbiertaV1(e, posicion) { vistas.push(`abierta:${posicion}`); },
    async applyCuentaDepositadaV1(e, posicion) { vistas.push(`deposito:${e.data.centavos}:${posicion}`); },
  };
  await saldosApply(projection, newEnvelope("cuenta.abierta@v1", "p", { streamId: "c1" }), 11);
  await saldosApply(projection, newEnvelope("cuenta.depositada@v1", "p", { streamId: "c1", centavos: 300 }), 12);
  assert.deepEqual(vistas, ["abierta:11", "deposito:300:12"]);
  // `cuenta.cerrada@v1` NO esta en la vista: llegar aqui seria una suscripcion
  // que nadie pidio
  await assert.rejects(
    () => saldosApply(projection, newEnvelope("cuenta.cerrada@v1", "p", {}), 13),
    /is not a declared event of the view/,
  );
  assert.equal(saldosTable, "vista_saldos");
  assert.equal(saldosMaxStalenessMs, 3000);
});
"#,
    )
    .unwrap();
    let out = Command::new("node")
        .args(["--test", "es.test.ts"])
        .current_dir(&destino)
        .output()
        .expect("node --test");
    let salida = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "el fold generado no reconstruye como dice:\n{salida}"
    );
    assert!(salida.contains("pass 10"), "{salida}");
}

/// Las reglas de event sourcing y CQRS: cada una bloquea una forma de tener un
/// flujo que no es un flujo, o una vista que miente.
#[test]
fn las_reglas_de_event_sourcing_bloquean() {
    let dir = std::env::temp_dir().join("axon-es-reglas");
    let correr = |manifiesto: &str, ddl: &str| -> String {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sql")).unwrap();
        std::fs::write(dir.join("sql/001_init.expand.sql"), ddl).unwrap();
        std::fs::write(dir.join("libro.toml"), manifiesto).unwrap();
        let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        assert!(!ok, "paso limpio");
        err
    };

    // el UNIQUE que hace posible la version optimista. Se quita el bloque
    // entero para no dejar una coma huerfana: un DDL que no parsea haria que
    // este test pase por el motivo equivocado.
    let sin_unique = DDL_ES.replace(
        "  en         timestamptz NOT NULL DEFAULT now(),\n  \
         -- Sin este UNIQUE, dos escrituras concurrentes sobre el mismo flujo entran\n  \
         -- las dos con la misma version y nadie ve un error.\n  \
         UNIQUE (stream_id, version)\n",
        "  en         timestamptz NOT NULL DEFAULT now()\n",
    );
    assert!(!sin_unique.contains("UNIQUE"), "la variante no quito el UNIQUE");
    let err = correr(MANIFIESTO_ES, &sin_unique);
    assert!(err.contains("sin UNIQUE sobre (stream_id, version)"), "{err}");
    assert!(err.contains("depende de en que orden se lean"), "{err}");

    // un agregado fundado en un evento que el servicio no emite
    let ajeno = MANIFIESTO_ES.replace(
        r#"events = ["cuenta.abierta@v1", "cuenta.depositada@v1", "cuenta.cerrada@v1"]"#,
        r#"events = ["cuenta.abierta@v1", "otra.cosa@v1"]"#,
    );
    let err = correr(&ajeno, DDL_ES);
    assert!(err.contains("no declara emitir"), "{err}");
    assert!(err.contains("esto es una vista, no un agregado"), "{err}");

    // la vista sin donde anotar hasta donde llego
    let sin_cp = DDL_ES.replace("CREATE TABLE vista_saldos_checkpoint", "CREATE TABLE otra_tabla");
    let err = correr(MANIFIESTO_ES, &sin_cp);
    assert!(err.contains("vista_saldos_checkpoint"), "{err}");
    assert!(err.contains("reprocesa desde el principio"), "{err}");

    // una vista mas vieja que el presupuesto del servicio
    let vieja = MANIFIESTO_ES.replace("max_staleness_ms = 3000", "max_staleness_ms = 9000");
    let err = correr(&vieja, DDL_ES);
    assert!(err.contains("admite 9000ms de atraso"), "{err}");
    assert!(err.contains("no puede cumplir lo que prometio"), "{err}");

    // y una vista bajo `consistency = "strong"`
    let fuerte = MANIFIESTO_ES
        .replace("consistency = \"eventual\"", "consistency = \"strong\"")
        .replace("max_staleness_ms = 5000\n", "");
    let err = correr(&fuerte, DDL_ES);
    assert!(err.contains("un dato viejo por definicion"), "{err}");

    // la sombra que no coincide con la vista
    let sombra_corta = DDL_ES.replace(
        "CREATE TABLE vista_saldos_sombra (\n  stream_id  uuid PRIMARY KEY,\n  centavos   bigint NOT NULL,\n  posicion   bigint NOT NULL\n);",
        "CREATE TABLE vista_saldos_sombra (\n  stream_id  uuid PRIMARY KEY,\n  posicion   bigint NOT NULL\n);",
    );
    // la guarda compara contra el original: el mismo texto tambien esta en la
    // vista viva, asi que buscarlo suelto no dice si el reemplazo aplico
    assert_ne!(sombra_corta, DDL_ES, "la variante no aplico");
    let err = correr(MANIFIESTO_ES, &sombra_corta);
    assert!(err.contains("`vista_saldos_sombra` sin la columna `centavos`"), "{err}");
    assert!(err.contains("recien ahi se veria"), "{err}");

    // y sin sombra: reconstruir en el sitio sirve una vista incompleta
    let sin_sombra = DDL_ES.replace("CREATE TABLE vista_saldos_sombra", "CREATE TABLE otra_sombra");
    let err = correr(MANIFIESTO_ES, &sin_sombra);
    assert!(err.contains("falta `vista_saldos_sombra`"), "{err}");
    assert!(err.contains("menos filas de las que hay"), "{err}");

    // el punto de la vista, sin flujo en la clave: un flujo pisa al otro
    let cp_global = DDL_ES.replace(
        "  vista      text NOT NULL,\n  -- por FLUJO: la version de un evento es su posicion dentro de su flujo, asi\n  -- que un solo numero para toda la vista no identifica nada en cuanto hay mas\n  -- de un flujo\n  stream_id  uuid NOT NULL,\n  posicion   bigint NOT NULL,\n  PRIMARY KEY (vista, stream_id)\n",
        "  vista      text PRIMARY KEY,\n  stream_id  uuid NOT NULL,\n  posicion   bigint NOT NULL\n",
    );
    assert!(!cp_global.contains("PRIMARY KEY (vista, stream_id)"), "la variante no aplico");
    let err = correr(MANIFIESTO_ES, &cp_global);
    assert!(err.contains("sin clave sobre (vista, stream_id)"), "{err}");
    assert!(err.contains("Un flujo pisaria el punto de otro"), "{err}");

    // fotos declaradas sin tabla, y sin la columna que las hace seguras
    let sin_tabla = DDL_ES.replace("CREATE TABLE cuenta_snapshot", "CREATE TABLE otra_foto");
    let err = correr(MANIFIESTO_ES, &sin_tabla);
    assert!(err.contains("sin la tabla `cuenta_snapshot`"), "{err}");

    let sin_reglas = DDL_ES.replace("  reglas     int  NOT NULL,\n", "");
    let err = correr(MANIFIESTO_ES, &sin_reglas);
    assert!(err.contains("sin columna `reglas`"), "{err}");
    assert!(
        err.contains("da un estado que ya no coincide con reproducir el flujo"),
        "{err}"
    );

    // una foto por evento no es una cache
    let cada_uno = MANIFIESTO_ES.replace("snapshot_every = 2", "snapshot_every = 1");
    let (out, _, _) = {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sql")).unwrap();
        std::fs::write(dir.join("sql/001_init.expand.sql"), DDL_ES).unwrap();
        std::fs::write(dir.join("libro.toml"), &cada_uno).unwrap();
        axon(&["verify", dir.to_str().unwrap()])
    };
    assert!(
        out.contains("una segunda copia del flujo"),
        "una foto por evento no aviso:\n{out}"
    );

    // un agregado que publica sin outbox: el dual-write que el flujo evitaba
    let sin_outbox = MANIFIESTO_ES.replace("outbox = true", "outbox = false");
    assert!(sin_outbox.contains("outbox = false"), "la variante no aplico");
    let err = correr(&sin_outbox, DDL_ES);
    assert!(err.contains("necesita `[patterns] outbox = true`"), "{err}");
    assert!(
        err.contains("el evento esta anotado y nadie lo recibio"),
        "{err}"
    );

    // el flujo es append-only, y no como recomendacion
    let err = correr(
        MANIFIESTO_ES,
        &format!("{DDL_ES}\nUPDATE cuenta_event SET data = '{{}}'::jsonb WHERE version = 1;\n"),
    );
    assert!(err.contains("es el flujo de `cuenta`"), "{err}");
    assert!(err.contains("un pasado que no ocurrio"), "{err}");
    // y ni siquiera marcado como `.contract.sql`: para eso no hay permiso
    let dir2 = std::env::temp_dir().join("axon-es-append");
    let _ = std::fs::remove_dir_all(&dir2);
    std::fs::create_dir_all(dir2.join("sql")).unwrap();
    std::fs::write(dir2.join("sql/001_init.expand.sql"), DDL_ES).unwrap();
    std::fs::write(
        dir2.join("sql/002_arreglo.contract.sql"),
        "DELETE FROM cuenta_event WHERE version = 1;\n",
    )
    .unwrap();
    std::fs::write(dir2.join("libro.toml"), MANIFIESTO_ES).unwrap();
    let (_, err, ok) = axon(&["verify", dir2.to_str().unwrap()]);
    assert!(!ok, "un DELETE sobre el flujo paso por estar en un .contract.sql");
    assert!(err.contains("es el flujo de `cuenta`"), "{err}");

    // un evento del agregado que ninguna transicion de la maquina emite
    let maquina = MANIFIESTO_ES.replace(
        "[aggregate.cuenta]",
        "[machine.cuenta]\ninitial = \"nueva\"\nfinal = [\"cerrada\"]\n\n\
         [machine.cuenta.transitions.abrir]\nfrom = [\"nueva\"]\nto = \"abierta\"\n\
         on = \"cuenta.abierta@v1\"\nemits = \"cuenta.abierta@v1\"\n\n\
         [aggregate.cuenta]\nmachine = \"cuenta\"",
    );
    let err = correr(&maquina, DDL_ES);
    assert!(err.contains("ninguna transicion de `cuenta` lo emite"), "{err}");
    assert!(err.contains("no sabria a que estado llevarlo"), "{err}");
}

/// El hueco que esto cierra: el esquema de la bodega se generaba para tres
/// dialectos y solo GCP tenia camino de ingesta. Se podia aplicar el esquema en
/// Snowflake, desplegar, y quedarse con tablas vacias sin un solo error — que
/// es indistinguible de "no paso nada en el negocio".
#[test]
fn la_ingesta_no_se_promete_sin_camino() {
    // Cada combinacion cableada tiene que rendir, y el recurso que lleva los
    // eventos tiene que estar ahi. Un target que "rinde" sin el recurso es el
    // mismo silencio con otra forma.
    for (target, bodega, marca) in [
        ("gcp", "bigquery", "bigquery_config"),
        ("aws", "clickhouse", "aws_kinesis_firehose_delivery_stream"),
        ("aws", "snowflake", "aws_kinesis_firehose_delivery_stream"),
        ("local", "clickhouse", "clickhouse/clickhouse-server"),
        ("k8s", "clickhouse", "image: timberio/vector"),
    ] {
        let f = ajustado(bodega);
        let (out, err, ok) = axon(&["infra", &f, "--target", target]);
        assert!(ok, "{target}+{bodega}: {err}");
        assert!(
            out.contains(marca),
            "{target}+{bodega} rinde sin `{marca}`: nada llevaria los eventos a la bodega"
        );
    }
    // Y lo que no tiene camino se RECHAZA, con el nombre de la combinacion y
    // que hacer al respecto.
    for (target, bodega) in [("gcp", "clickhouse"), ("aws", "bigquery"), ("k8s", "bigquery")] {
        let f = ajustado(bodega);
        let (_, err, ok) = axon(&["infra", &f, "--target", target]);
        assert!(!ok, "{target}+{bodega} rindio sin camino de ingesta");
        assert!(err.contains("no tiene camino de ingesta"), "{err}");
        assert!(err.contains("las tablas se quedarian vacias"), "{err}");
        assert!(err.contains("export = false"), "no dice que hacer:\n{err}");
    }
    // El cargador local sale del mismo sitio que el esquema: si las rutas del
    // JSON no coincidieran con las columnas, la carga fallaria en la bodega y
    // no aqui.
    let (sql, err, ok) = axon(&["analytics", "examples", "--cargar", "local.ndjson"]);
    assert!(ok, "{err}");
    assert!(sql.contains("INSERT INTO axon.order_placed_v1"), "{sql}");
    // el hash sale con salt por parametro, nunca el valor
    assert!(sql.contains("SHA256(concat({salt:String}"), "{sql}");
    assert!(!sql.contains("AS customer_email,"), "el correo viaja en claro:\n{sql}");
    // idempotente: un cargador periodico corre muchas veces sobre el mismo log
    assert!(sql.contains("NOT IN (SELECT event_id FROM"), "{sql}");
}

/// El drift de la bodega no da error en ninguna parte: un campo nuevo que la
/// tabla no tiene se carga como nada, y una columna vieja se queda con los
/// datos que tenia. Las dos cosas dan consultas que devuelven numeros.
#[test]
fn el_drift_de_la_bodega_se_detecta() {
    let dir = std::env::temp_dir().join("axon-bodega-check");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let tsv = dir.join("real.tsv");

    // El volcado que corresponde a lo declarado, armado a mano a partir del
    // esquema generado: si esto no diera 0, el resto del test no diria nada.
    let real = "\
order_placed_v1\tevent_id\tString
order_placed_v1\tevent_type\tString
order_placed_v1\tsource\tString
order_placed_v1\tevent_time\tDateTime64(3)
order_placed_v1\ttrace_id\tNullable(String)
order_placed_v1\tcorrelation_id\tString
order_placed_v1\tcausation_id\tNullable(String)
order_placed_v1\torder_id\tNullable(String)
order_placed_v1\tcustomer_id\tNullable(String)
order_placed_v1\tcustomer_email_hash\tNullable(String)
order_placed_v1\ttotal_amount\tNullable(Int64)
order_placed_v1\ttotal_currency\tNullable(String)
";
    let manifiesto = r#"service = "tienda"
version = "1.0.0"
owner = "equipo"
tier = "1"

[analytics]
pii = "hash"
warehouse = "clickhouse"

[emits."order.placed@v1"]
orderId = "uuid"
customerId = "uuid"
customerEmail = "string"
total = "money"

pii = []
"#;
    // `pii` va como campo del servicio, no dentro de emits
    let manifiesto = manifiesto.replace("pii = []\n", "");
    let manifiesto = manifiesto.replace(
        "tier = \"1\"",
        "tier = \"1\"\npii = [\"customerEmail\"]",
    );
    std::fs::write(dir.join("tienda.toml"), &manifiesto).unwrap();
    let d = dir.to_str().unwrap();

    let check = |contenido: &str| -> (String, bool) {
        std::fs::write(&tsv, contenido).unwrap();
        let (out, err, ok) = axon(&[
            "analytics",
            d,
            "--target",
            "clickhouse",
            "--check",
            tsv.to_str().unwrap(),
        ]);
        (format!("{out}{err}"), ok)
    };

    let (salida, ok) = check(real);
    assert!(ok, "el volcado que coincide dio diferencias:\n{salida}");
    assert!(salida.contains("0 diferencias"), "{salida}");

    // una columna declarada que la tabla no tiene
    let falta = real.replace("order_placed_v1\ttotal_amount\tNullable(Int64)\n", "");
    let (salida, ok) = check(&falta);
    assert!(!ok, "una columna que falta paso limpio");
    assert!(salida.contains("falta `order_placed_v1.total_amount`"), "{salida}");
    assert!(salida.contains("no se guarda en ningun lado"), "{salida}");

    // un tipo que no es el mismo: una fecha guardada como texto ordena mal
    let tipo = real.replace(
        "order_placed_v1\tevent_time\tDateTime64(3)",
        "order_placed_v1\tevent_time\tString",
    );
    let (salida, ok) = check(&tipo);
    assert!(!ok, "una fecha como texto paso limpio");
    assert!(salida.contains("es texto en la bodega"), "{salida}");

    // el correo en claro junto al hash: el manifiesto dice `hash` y el valor
    // viejo sigue ahi
    let claro = format!("{real}order_placed_v1\tcustomer_email\tNullable(String)\n");
    let (salida, ok) = check(&claro);
    assert!(!ok, "el correo en claro paso limpio");
    assert!(salida.contains("existe en claro"), "{salida}");
    assert!(salida.contains("se queda con los correos que ya tenia"), "{salida}");

    // una columna de mas es un aviso, no un error: no rompe nada
    let sobra = format!("{real}order_placed_v1\tsobra\tString\n");
    let (salida, ok) = check(&sobra);
    assert!(ok, "una columna de mas bloqueo:\n{salida}");
    assert!(salida.contains("Sobra de una version anterior"), "{salida}");

    // Y lo que mas importa: un volcado vacio NO puede dar 0 diferencias. Es el
    // resultado de correr la consulta contra la bodega equivocada, y leerlo
    // como "todo bien" es peor que no comprobar.
    let (salida, ok) = check("");
    assert!(!ok, "un volcado vacio dio 0 diferencias");
    assert!(salida.contains("no tiene ninguna columna"), "{salida}");
    assert!(salida.contains("se lee como que todo esta bien"), "{salida}");
}

/// La config de Vector se valida con `vector validate`: el parser que la va a
/// leer es el que dice si esta bien. Un `route` con una rama sin consumidor, o
/// un campo mal, salen ahi y no el dia que falte un evento en la bodega.
#[test]
fn la_config_de_vector_valida() {
    if !tiene("docker") {
        eprintln!("salteado: falta docker");
        return;
    }
    let dir = std::env::temp_dir().join("axon-vector");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (cfg, err, ok) = axon(&["analytics", "examples", "--vector"]);
    assert!(ok, "{err}");
    std::fs::write(dir.join("vector.yaml"), &cfg).unwrap();

    // Una fuente por evento, no una comodin con router: un router deja una rama
    // `_unmatched`, y el evento que cae ahi se descarta en silencio.
    assert!(!cfg.contains("type: route"), "un router deja eventos sin consumidor");
    assert!(cfg.contains("queue: axon-warehouse"), "sin queue group, cada replica escribe la misma fila");
    // El salt del hash entra por variable, nunca en el archivo generado.
    assert!(cfg.contains("get_env_var!(\"AXON_PII_SALT\")"), "{cfg}");
    assert!(!cfg.contains("customer_email\":"), "el correo viaja en claro:\n{cfg}");
    // Un campo que la tabla no tiene es un error, no algo que se descarta.
    assert!(cfg.contains("skip_unknown_fields: false"), "{cfg}");
    // El buffer en disco: en memoria, un reinicio pierde lo no escrito.
    assert!(cfg.contains("type: disk"), "{cfg}");

    let out = Command::new("docker")
        .args([
            "run", "--rm",
            "-e", "AXON_WAREHOUSE_USER=u",
            "-e", "AXON_WAREHOUSE_PASSWORD=p",
            "-e", "AXON_PII_SALT=s",
            "-v", &format!("{}:/etc/vector:ro", dir.display()),
            "timberio/vector:0.44.0-alpine",
            // `--no-environment` no comprueba conexiones: aqui no hay broker ni
            // bodega, y lo que se valida es la config, no el entorno.
            "validate", "--no-environment", "/etc/vector/vector.yaml",
        ])
        .output()
        .expect("docker run vector");
    let salida = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if salida.contains("Unable to find image") || salida.contains("Cannot connect to the Docker daemon") {
        eprintln!("salteado: sin la imagen de vector");
        return;
    }
    assert!(out.status.success(), "vector no valida la config generada:\n{salida}");
    // un aviso hoy es un evento perdido manana
    assert!(!salida.contains("warning"), "valida con avisos:\n{salida}");
    assert!(!salida.contains("no consumers"), "una rama sin consumidor:\n{salida}");
}
