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
        "service = \"ledger\"\nowner = \"x\"\ntier = \"0\"\n[infra]\nstate = \"postgres\"\nmigrations = \"sql/\"\n",
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

#[test]
fn el_hcl_generado_parsea() {
    if !tiene("terraform") {
        eprintln!("salteado: terraform no esta instalado");
        return;
    }
    for target in ["gcp", "aws"] {
        let dir = std::env::temp_dir().join(format!("axon-tf-{target}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (tf, _, _) = axon(&["infra", "examples", "--target", target]);
        std::fs::write(dir.join("main.tf"), tf).unwrap();
        let out = Command::new("terraform")
            .args(["fmt", "-check", dir.to_str().unwrap()])
            .output()
            .expect("terraform");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!err.to_lowercase().contains("error"), "{target}:\n{err}");
    }
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
