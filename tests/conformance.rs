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

#[test]
fn el_mismo_plan_en_cuatro_targets() {
    for (target, marca) in [
        ("local", "postgres:16-alpine"),
        ("gcp", "google_pubsub_subscription"),
        ("aws", "aws_sqs_queue"),
        ("k8s", "kind: Trigger"),
    ] {
        let (out, err, ok) = axon(&["infra", "examples", "--target", target]);
        assert!(ok, "{target}: {err}");
        assert!(out.contains(marca), "{target} no genero {marca}");
    }
    // DLQ siempre, en todos los targets
    for t in ["gcp", "aws", "k8s"] {
        let (out, _, _) = axon(&["infra", "examples", "--target", t]);
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
        let (out, _, _) = axon(&["infra", "examples", "--target", target]);
        assert!(out.contains(marca), "{target} no despliega el workload");
    }
    // y la entrega llega a alguien: nada de suscripciones al vacio
    let (gcp, _, _) = axon(&["infra", "examples", "--target", "gcp"]);
    assert!(gcp.contains("push_endpoint = google_cloud_run_v2_service.payments.uri"));
    let (k, _, _) = axon(&["infra", "examples", "--target", "k8s"]);
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
    assert!(err.contains("solo hay `container`"), "{err}");
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
        "sin `owner`",
        "sin `tier`",
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
    for (target, provider, vars) in casos {
        let dir = std::env::temp_dir().join(format!("axon-tf-{target}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (tf, _, _) = axon(&["infra", "examples", "--target", target]);
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
        err.contains("sin `owner`"),
        "TODO se acepto como owner:\n{err}"
    );
    assert!(err.contains("sin `tier`"), "{err}");
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
        let (out, _, _) = axon(&["infra", "examples", "--target", target]);
        assert!(out.contains(marca), "{target} no genero el edge ({marca})");
    }
    // auth y rate limit llegan a la configuracion, no se quedan en el manifiesto
    let (k, _, _) = axon(&["infra", "examples", "--target", "k8s"]);
    assert!(k.contains("axon.dev/auth: public"), "{k}");
    assert!(k.contains("axon.dev/rate-limit: \"60\""), "{k}");
    assert!(
        k.contains("timeouts: { request: 5s }"),
        "el timeout del edge no llego"
    );
    let (a, _, _) = axon(&["infra", "examples", "--target", "aws"]);
    assert!(
        a.contains("authorization_type = \"JWT\""),
        "ruta privada sin authorizer"
    );
    assert!(
        a.contains("authorization_type = \"NONE\""),
        "ruta publica mal marcada"
    );

    // publico implica CDN; privado implica que no la lleva
    let (g, _, _) = axon(&["infra", "examples", "--target", "gcp"]);
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
        "[A01] mal.pagar: ruta publica que muta en un servicio tier 0",
        "[A04] mal.pagar: ruta publica sin `timeout_ms`",
        "[A09] mal.perfil: devuelve `email`, declarado PII",
        "[A02] mal: `sk_ESTO_NO_ES_UNA_LLAVE`",
        "[A05] mal: bucket `abierto` publico y sin `retention_days`",
    ] {
        assert!(todo.contains(regla), "falto `{regla}`:\n{todo}");
    }

    // A05: el endurecimiento va generado, no recordado
    let (k, _, _) = axon(&["infra", "examples", "--target", "k8s"]);
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
    let (g, _, _) = axon(&["infra", "examples", "--target", "gcp"]);
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
        ts.contains("export const camposPII = [\"customer_email\"]"),
        "{ts}"
    );
    assert!(ts.contains("export function redactar"), "{ts}");
    // El mismo concepto se declara UNA vez: `customer_email` en el manifiesto
    // cubre `customerEmail` en el contrato y `customer-email` en una cabecera.
    // Antes hacia falta declararlo dos veces, que es absurdo.
    assert!(
        ts.contains("const normalizarPII"),
        "el redactor compara claves exactas:\n{ts}"
    );
    assert!(ts.contains("pii.has(normalizarPII(k))"), "{ts}");
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
        "http = \"POST /v1/orders\"",
        "http = \"POST /v1/pedidos\"",
    );
    assert!(err.contains("la ruta cambio de `POST /v1/orders`"), "{err}");
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
            r#"conPolitica("orders.getOrder", { timeoutMs: 1000, reintentos: 3, breaker: true }"#
        ),
        "la politica no salio del manifiesto:\n{ts}"
    );
    // reintentar sin llave de idempotencia duplica el efecto del otro lado
    assert!(ts.contains(r#"cabeceras(e, true)"#));
    // CAP: el lado declarado decide el aislamiento
    assert!(
        ts.contains(r#"export const nivelAislamiento = "SERIALIZABLE""#),
        "{ts}"
    );
    let (o, _, _) = axon(&["build", "examples/orders.toml", "examples"]);
    assert!(
        o.contains(r#"export const nivelAislamiento = "READ COMMITTED""#),
        "{o}"
    );
    assert!(
        o.contains("export const obsolescenciaMaximaMs = 3000"),
        "{o}"
    );
    // `degrade` obliga a pasar el camino degradado; `reject` no lo admite
    assert!(
        o.contains("respaldo: () => Promise<PaymentsCapturePaymentOut>"),
        "declarar degrade no obligo a un respaldo:\n{o}"
    );
    assert!(
        !ts.contains("respaldo: () =>"),
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
import { conPolitica, cabeceras, newEnvelope, ErrorAgotado, ErrorCircuitoAbierto } from "./contracts.ts";

test("el timeout corta", async () => {
  await assert.rejects(
    () => conPolitica("x.lento", { timeoutMs: 50, reintentos: 0, breaker: false },
      () => new Promise((r) => setTimeout(r, 5000))),
    ErrorAgotado,
  );
});

test("reintenta hasta que sale bien", async () => {
  let n = 0;
  const r = await conPolitica("x.flaky", { timeoutMs: 500, reintentos: 3, breaker: false }, async () => {
    if (++n < 3) throw new Error("boom");
    return "ok";
  });
  assert.equal(r, "ok");
  assert.equal(n, 3);
});

test("el circuito se abre y deja de golpear", async () => {
  let llamadas = 0;
  const caer = () => conPolitica("x.caido", { timeoutMs: 100, reintentos: 0, breaker: true },
    async () => { llamadas++; throw new Error("caido"); });
  for (let i = 0; i < 5; i++) await assert.rejects(caer);
  assert.equal(llamadas, 5);
  await assert.rejects(caer, ErrorCircuitoAbierto);
  assert.equal(llamadas, 5, "siguio golpeando con el circuito abierto");
});

test("la traza y la llave de idempotencia viajan en la llamada", () => {
  const e = newEnvelope("x@v1", "prueba", {});
  const h = cabeceras(e, true);
  assert.equal(h.traceparent, e.traceparent);
  assert.equal(h["x-correlation-id"], e.correlationId);
  assert.equal(h["x-causation-id"], e.id);
  assert.equal(h["idempotency-key"], e.id);
  assert.equal(cabeceras(e, false)["idempotency-key"], undefined);
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
        let (out, err, ok) = axon(&["infra", "examples", "--target", target]);
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
        let (o, _, _) = axon(&["infra", "examples", "--target", target]);
        assert!(
            o.contains(endpoint),
            "{target} sin destino OTLP configurable"
        );
    }

    // el muestreo sale del tier: un tier 0 se traza entero, porque cuando se
    // cae la traza que falta es justo la que hacia falta
    let (g, _, _) = axon(&["infra", "examples", "--target", "gcp"]);
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

    // alta disponibilidad no es respaldo: el standby replica el DROP TABLE
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
    let (g, _, _) = axon(&["infra", "examples", "--target", "gcp"]);
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
    let (a, _, _) = axon(&["infra", "examples", "--target", "aws"]);
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
        ts.contains(r#"export const rutasHttp = ["POST /v1/orders", "GET /v1/orders/{orderId}"]"#),
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
    assert!(msg.contains("sin `expires`"), "{msg}");

    // uno vencido no se ignora: se limpia o se renueva
    let (msg, ok) = probar(&format!(
        "{base}[flags.viejo]\nowner = \"e\"\nexpires = \"2024-01-15\"\n"
    ));
    assert!(!ok);
    assert!(msg.contains("vencio el 2024-01-15"), "{msg}");

    // sin dueno, nadie lo apaga
    let (msg, ok) = probar(&format!(
        "{base}[flags.huerfano]\nexpires = \"2099-01-01\"\n"
    ));
    assert!(!ok);
    assert!(msg.contains("flag sin `owner`"), "{msg}");

    // un rollout por peticion deja la misma entidad a medio migrar
    let (msg, ok) = probar(&format!(
        "{base}[flags.parcial]\nowner = \"e\"\nexpires = \"2099-01-01\"\nrollout = 25\n"
    ));
    assert!(!ok);
    assert!(msg.contains("rollout al 25% sin `sticky_by`"), "{msg}");

    // un kill switch se apaga entero: el error es propio, no el del sticky
    let (msg, ok) = probar(&format!(
        "{base}[flags.cortar]\nowner = \"e\"\nkill_switch = true\nrollout = 50\n"
    ));
    assert!(!ok);
    assert!(msg.contains("`kill_switch` con `rollout`"), "{msg}");

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
    assert!(msg.contains("no esta en `variants`"), "{msg}");

    // OpenFeature resuelve un tipo por flag, no uno por variante
    let (msg, ok) = probar(&format!(
        "{base}[flags.mezcla]\nowner = \"e\"\nkill_switch = true\n\
         default_variant = \"a\"\nvariants = {{ a = \"x\", b = 2 }}\n"
    ));
    assert!(!ok);
    assert!(msg.contains("mezclan tipos"), "{msg}");
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
    let (g, _, _) = axon(&["infra", "examples", "--target", "gcp"]);
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
