//! Un solo archivo de checks: si algo de esto se rompe, la herramienta miente.
use std::process::Command;

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
    let (ts, _, _) = axon(&["build", "examples/payments.toml"]);
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

#[test]
fn maquinas_de_estado() {
    let (ts, _, _) = axon(&["build", "examples/payments.toml"]);
    assert!(ts.contains(r#"export type PaymentState = "pending" | "captured" | "failed" | "refunded""#), "{ts}");
    assert!(ts.contains("paymentNext(state: PaymentState"));
    let (d, _, _) = axon(&["states", "examples"]);
    assert!(d.contains("pending --> captured: capture"));

    // deadlock y disparador fantasma tienen que bloquear
    let dir = std::env::temp_dir().join("axon-machine");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("m.toml"), r#"
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
"#).unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("no es ni metodo ni evento consumido"), "{err}");
    assert!(err.contains("deadlock"), "{err}");
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
