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
