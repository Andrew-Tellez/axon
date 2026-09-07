//! One single file of checks: if any of this breaks, the tool lies.
use std::process::Command;

fn has(bin: &str) -> bool {
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

/// The example, but with no `[pooler]`.
///
/// `orders` declares 4 shard nodes and today that is only brought up on the
/// local target: the rest REFUSE the plan instead of emitting a single
/// instance, which would apply with no error and leave the sharding
/// non-existent. So everything asserted about gcp, aws and k8s is asserted
/// over this copy —and `sharding_is_not_rendered_where_it_does_not_exist`
/// covers the refusal.
/// The example with no `[pooler]` and with the warehouse the target can feed.
///
/// `axon infra` refuses a warehouse with no ingest path, and rightly so: the
/// schema would apply and the tables would stay empty without a single error.
/// The example declares ClickHouse, which is the one with a path on local; for
/// gcp it has to say BigQuery, and k8s has none yet.
fn tuned(warehouse: &str) -> String {
    // one directory per call: the tests run in parallel and sharing the path
    // makes one delete the tree another is reading
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "axon-no-pooler-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for e in std::fs::read_dir("examples").unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if e.path().is_dir() {
            if name == "sql" || name == "sql-policies" {
                copy_tree(&e.path(), &dir.join(&name));
            }
            continue;
        }
        if !name.ends_with(".toml") && !name.ends_with(".json") {
            continue;
        }
        let mut text = std::fs::read_to_string(e.path()).unwrap();
        text = match warehouse {
            // k8s has no ingest path: what fits is exactly what the error
            // message says, `export = false`
            "none" => text.replace("[analytics]", "[analytics]\nexport = false"),
            other => text.replace(
                "warehouse = \"clickhouse\"",
                &format!("warehouse = \"{other}\""),
            ),
        };
        // the block goes at the end of the manifest, so cutting from there is
        // enough and no test has to parse TOML
        let stripped = match text.find("\n[pooler]") {
            Some(i) => text[..i].to_string(),
            None => text,
        };
        std::fs::write(dir.join(&name), stripped).unwrap();
    }
    dir.to_string_lossy().to_string()
}

/// Where the plan for this target comes from: the example as-is where the
/// sharding renders, and the pooler-less copy where it does not yet.
fn source_for(target: &str) -> String {
    match target {
        "local" | "plan" => "examples".to_string(),
        "gcp" => tuned("bigquery"),
        "k8s" => tuned("none"),
        _ => tuned("clickhouse"),
    }
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        if e.path().is_dir() {
            copy_tree(&e.path(), &to.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

/// What this refusal prevents: a `terraform apply` with no error and a single
/// Postgres where the manifest declares four. The sharding would not exist and
/// nothing would say so.
#[test]
fn sharding_is_not_rendered_where_it_does_not_exist() {
    for t in ["gcp", "aws", "k8s"] {
        let (_, err, ok) = axon(&["infra", "examples", "--target", t]);
        assert!(!ok, "{t} rendered a plan with sharding it cannot shard");
        assert!(err.contains("shards = 4"), "{t}: {err}");
        assert!(err.contains("--target local"), "{t}: {err}");
    }
    // and with no pooler —and a warehouse the target can feed— all three
    // still render
    for t in ["gcp", "aws", "k8s"] {
        let (_, err, ok) = axon(&["infra", &source_for(t), "--target", t]);
        assert!(ok, "{t}: {err}");
    }
}

#[test]
fn the_examples_are_clean() {
    let (out, err, ok) = axon(&["verify", "examples"]);
    assert!(ok, "verify fallo: {err}");
    assert!(out.contains("0 errors"), "{out}");
}

#[test]
fn traceability_is_not_optional() {
    let (ts, _, _) = axon(&["build", "examples/payments.toml", "examples"]);
    for f in ["traceparent", "correlationId", "causationId"] {
        assert!(ts.contains(f), "falta {f}");
    }
    // outbox declared -> the emitter does not touch the bus (no dual-write)
    assert!(ts.contains("this.outbox.stage(newEnvelope"));
    assert!(!ts.contains("this.bus.publish(newEnvelope"));
    // And the caller's transaction is MANDATORY: on a connection of its own,
    // the `stage` commits by itself, so a rolled-back transaction leaves the
    // event with no row and the relay publishes something that never happened.
    // Measured against the containers before fixing it: 0 payments and 1 event.
    assert!(
        ts.contains("tx: unknown, cause?: Envelope<unknown>"),
        "the emitter with an outbox does not require the transaction:\n{ts}"
    );
    assert!(
        ts.contains("stage(newEnvelope(\"payment.captured@v1\", \"payments\", data, cause), tx)")
    );
    // with no outbox there is no transaction to share, and asking for one would be noise
    let (without, _, _) = axon(&["build", "examples/orders.toml", "examples"]);
    assert!(without.contains("this.bus.publish(newEnvelope"));
    assert!(
        !without.contains("tx: unknown"),
        "a service with no outbox has no transaction to pass"
    );
    // an idempotent consumer by default, not by discipline
    assert!(ts.contains("this.inbox.once(e.id"));
    assert!(ts.contains(r#"case "order.placed@v1""#));
}

#[test]
fn migrations_folded_into_the_er_diagram() {
    let (er, _, _) = axon(&["er", "examples"]);
    assert!(er.contains("ORDER ||--o{ ORDER_ITEM : order_id"));
    assert!(
        er.contains("text provider_ref"),
        "ADD COLUMN was not folded"
    );
    assert!(!er.contains("currency"), "DROP COLUMN was not folded");
}

/// The schema is read with a SQL parser, not with a regex. What follows is
/// exactly what the regex could not do, and its worst property was breaking in
/// silence: returning the wrong columns with nobody finding out.
#[test]
fn the_ddl_is_really_parsed() {
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
    // PRIMARY KEY and FOREIGN KEY declared at table level, not on the column
    assert!(
        er.contains("uuid id PK"),
        "table-level PK not resolved:\n{er}"
    );
    assert!(
        er.contains("uuid account_id FK"),
        "table-level FK not resolved:\n{er}"
    );
    assert!(
        er.contains("ACCOUNT ||--o{ LEDGER_ENTRY : account_id"),
        "{er}"
    );
    // a type has to fit in one token or it breaks mermaid's ER diagram
    assert!(
        er.contains("numeric(20,4) amount_cents") || er.contains("numeric(20,_4) amount_cents"),
        "{er}"
    );
    assert!(er.contains("varchar(64) handle"), "{er}");
    // CREATE INDEX is not a table
    assert!(
        !er.to_uppercase().contains("LEDGER_ENTRY_ACCOUNT_IDX"),
        "{er}"
    );

    // a DROP inside a comment is not destructive; a real one is
    std::fs::write(
        dir.join("sql/002_no_es_drop.expand.sql"),
        "-- ojo: no hacer DROP TABLE account aqui\nALTER TABLE account ADD COLUMN nota text;\n",
    )
    .unwrap();
    let (_, _, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "a DROP inside a comment counted as destructive");

    std::fs::write(
        dir.join("sql/003_si_es_drop.expand.sql"),
        "ALTER TABLE account DROP COLUMN nota;\n",
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("destructive migration not marked"), "{err}");

    // and SQL that cannot be parsed fails loudly, never in silence
    std::fs::write(
        dir.join("sql/004_roto.expand.sql"),
        "CREATE TABL account (;\n",
    )
    .unwrap();
    let (_, err, ok) = axon(&["er", dir.to_str().unwrap()]);
    assert!(!ok, "the invalid SQL was ignored in silence");
    assert!(err.contains("could not parse the SQL"), "{err}");
}

/// A key added in a LATER migration has to count. It was invisible, and with
/// that every uniqueness rule —the event stream's, the sharding ones, a view's
/// checkpoint— took it as absent and passed in silence.
#[test]
fn a_key_added_later_counts() {
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
    // the ER diagram is the simplest projection of the folded schema
    let (er, err, ok) = axon(&["er", dir.to_str().unwrap()]);
    assert!(ok, "{err}");
    // the added column is there, and the PK is marked
    assert!(er.contains("stream_id"), "{er}");
    assert!(
        er.contains("PK"),
        "the PK added later was not marked:\n{er}"
    );
}

/// A rename in a LATER migration has to count. It was invisible, and with that
/// every rule about the renamed table went on checking a table that no longer
/// exists —and passed, because the old one still had everything it asked for.
#[test]
fn a_later_rename_counts() {
    let dir = std::env::temp_dir().join("axon-alter-rename");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sql")).unwrap();
    std::fs::write(
        dir.join("sql/001_init.expand.sql"),
        "CREATE TABLE punto (vista text NOT NULL, stream_id uuid NOT NULL, \
         posicion bigint NOT NULL, PRIMARY KEY (vista, stream_id));\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("sql/002_rename.expand.sql"),
        "ALTER TABLE punto RENAME TO point;\n\
         ALTER TABLE point RENAME COLUMN vista TO view_name;\n\
         ALTER TABLE point RENAME COLUMN posicion TO position;\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("s.toml"),
        "service = \"s\"\nversion = \"1.0.0\"\nowner = \"e\"\ntier = \"1\"\n\n\
         [analytics]\nexport = false\n\n[infra]\nstate = \"postgres\"\nmigrations = \"sql/\"\n",
    )
    .unwrap();
    let (er, err, ok) = axon(&["er", dir.to_str().unwrap()]);
    assert!(ok, "{err}");
    assert!(
        er.contains("POINT {"),
        "the renamed table does not show up:\n{er}"
    );
    assert!(
        !er.to_lowercase().contains("punto"),
        "the old table is still in the schema:\n{er}"
    );
    assert!(
        er.contains("view_name"),
        "the renamed column does not show up:\n{er}"
    );
    assert!(
        !er.contains("vista"),
        "the old column is still there:\n{er}"
    );
    // the PK survives the rename, under the new name
    assert!(er.contains("PK"), "the PK was lost in the rename:\n{er}");
}

/// The route the code serves and the one the scheduler hits have to be THE
/// SAME. They were apart —the route in English and the cron still in Spanish—
/// and the CronJob applied with no error against a 404: the sweep simply
/// stopped running, and the only thing that said so was a curl swallowing the
/// failure.
#[test]
fn the_cron_hits_the_route_the_code_serves() {
    let (ts, err, ok) = axon(&["build", "examples/checkout.toml", "examples"]);
    assert!(ok, "{err}");
    // the internal routes, as the generated code declares them
    let routes: Vec<String> = ts
        .lines()
        .filter(|l| l.contains("Route") && l.contains("POST /internal/"))
        .map(|l| {
            l.split("POST ")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    assert!(
        routes.len() >= 3,
        "the example should generate a sweep, a prune and a rebuild: {routes:?}"
    );
    // the neutral plan is the source of all four targets: if the route matches
    // here, it matches on all four
    let (plan, err, ok) = axon(&["infra", "examples", "--target", "plan"]);
    assert!(ok, "{err}");
    for r in &routes {
        // the rebuild carries no cron on purpose: it is not periodic
        if r.contains("/rebuild") {
            assert!(!plan.contains(r), "the rebuild should not carry a cron");
            continue;
        }
        assert!(
            plan.contains(r),
            "the code serves `{r}` and no cron in the plan hits it:\n{plan}"
        );
    }
    // and the other way round: no cron points at a route nobody serves
    for l in plan.lines().filter(|l| l.contains("/internal/")) {
        let r = l.split('"').find(|s| s.starts_with("/internal/")).unwrap();
        assert!(
            routes.iter().any(|x| x == r),
            "the cron hits `{r}` and the generated code does not serve it"
        );
    }
}

#[test]
fn the_same_plan_on_four_targets() {
    for (target, marker) in [
        ("local", "postgres:16-alpine"),
        ("gcp", "google_pubsub_subscription"),
        ("aws", "aws_sqs_queue"),
        ("k8s", "kind: Trigger"),
    ] {
        let (out, err, ok) = axon(&["infra", &source_for(target), "--target", target]);
        assert!(ok, "{target}: {err}");
        assert!(out.contains(marker), "{target} no genero {marker}");
    }
    // DLQ always, on every target
    for t in ["gcp", "aws", "k8s"] {
        let (out, _, _) = axon(&["infra", &source_for(t), "--target", t]);
        assert!(out.to_lowercase().contains("dead"), "{t} with no DLQ");
    }
}

#[test]
fn every_target_deploys_the_workload() {
    // without this the IaC leaves topics and databases with nothing running the code
    for (target, marker) in [
        ("local", "dockerfile: services/payments/Dockerfile"),
        (
            "gcp",
            "resource \"google_cloud_run_v2_service\" \"payments\"",
        ),
        ("aws", "resource \"aws_ecs_service\" \"payments\""),
        ("k8s", "kind: Deployment"),
    ] {
        let (out, _, _) = axon(&["infra", &source_for(target), "--target", target]);
        assert!(
            out.contains(marker),
            "{target} does not deploy the workload"
        );
    }
    // and delivery reaches somebody: no subscriptions into the void
    let (gcp, _, _) = axon(&["infra", &source_for("gcp"), "--target", "gcp"]);
    assert!(gcp.contains("push_endpoint = google_cloud_run_v2_service.payments.uri"));
    let (k, _, _) = axon(&["infra", &source_for("k8s"), "--target", "k8s"]);
    assert!(
        k.contains("kind: Service\nmetadata:\n  name: payments"),
        "the Trigger points at a Service that does not exist"
    );
    // the secret reaches the container, not just the vault
    assert!(gcp.contains("STRIPE_API_KEY"));
    let (loc, _, _) = axon(&["infra", "examples", "--target", "local"]);
    assert!(loc.contains("DATABASE_URL: postgres://postgres:local@db-payments"));
}

#[test]
fn an_unknown_runtime_is_not_ignored() {
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
fn environments_are_deltas() {
    let (prod, _, _) = axon(&["infra", "examples", "--target", "plan", "--env", "prod"]);
    let (stg, _, _) = axon(&["infra", "examples", "--target", "plan", "--env", "staging"]);
    assert!(
        prod.contains("\"min_instances\": 3"),
        "prod did not apply the override"
    );
    assert!(!stg.contains("\"min_instances\": 3"));
}

#[test]
fn the_expected_and_the_real_sequence() {
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
fn api_and_governance_block() {
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
    for expected in [
        "no `owner`",
        "no `tier`",
        "has no version in the path",
        "mutates with no `idempotent = true`",
        "no `timeout_ms`",
    ] {
        assert!(
            err.contains(expected),
            "the `{expected}` check is missing:\n{err}"
        );
    }
}

/// What the README promises and only an external tool can confirm.
/// Without the tool, the test skips instead of lying.
#[test]
fn the_generated_typescript_typechecks() {
    if !has("node") {
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

/// The type of a consumed event is declared by its emitter. Without the other
/// manifests, `build` has to fail with a useful message, not generate code
/// that does not compile.
#[test]
fn build_without_sources_fails_clearly() {
    let (_, err, ok) = axon(&["build", "examples/payments.toml"]);
    assert!(!ok);
    assert!(err.contains("whoever emits it was not found"), "{err}");
    assert!(err.contains("Pass the other manifests"), "{err}");
}

/// `terraform fmt` only says the HCL parses. `validate` with the real
/// providers says the attributes exist —which is what catches an
/// interpolation of a non-existent variable or a block missing a field.
#[test]
fn the_generated_hcl_validates() {
    if !has("terraform") {
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
    // And the same two targets over a manifest WITH a saga: the sweep emits a
    // scheduler and a task the example does not have, and an invented
    // attribute there goes unseen until the `apply`.
    //
    // The sweep's own variables are declared by axon, so they do NOT go here:
    // declaring them on both sides is a `Duplicate variable declaration`, and
    // the fallback to `fmt` does not see it.
    let saga = fixture_saga("tf").to_string_lossy().to_string();
    let mut casos: Vec<(String, &str, String, String)> = casos
        .iter()
        .map(|(t, prov, vars)| (t.to_string(), *prov, vars.to_string(), source_for(t)))
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
        // the sweep's variables are declared by axon: none goes here
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

        // with no network the providers cannot be downloaded: it falls back to
        // `fmt`, which at least confirms the HCL parses
        let init = Command::new("terraform")
            .args(["init", "-backend=false", "-input=false"])
            .current_dir(&dir)
            .output()
            .expect("terraform init");
        if !init.status.success() {
            eprintln!("{target}: with no providers, only the parse is validated");
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
        let printed = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(out.status.success(), "{target} no valida:\n{printed}");
        // a warning today is a provider error tomorrow
        assert!(
            !printed.contains("Warning:"),
            "{target} validates with warnings:\n{printed}"
        );
    }
}

/// The suite validated the YAML axon generates and never the repo's own. A
/// workflow that does not parse does not fail: GitHub reports it with its path
/// as the name and no jobs at all, which is to say a broken release looks
/// like it never ran.
#[test]
fn the_repo_workflows_parse() {
    let dir = std::path::Path::new(".github/workflows");
    let mut seen = 0;
    for e in std::fs::read_dir(dir).expect("workflows") {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "yml" && x != "yaml") {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        let doc: Result<serde_yaml_ng::Value, _> = serde_yaml_ng::from_str(&text);
        assert!(doc.is_ok(), "{}: {}", p.display(), doc.unwrap_err());
        let doc = doc.unwrap();
        assert!(doc.get("jobs").is_some(), "{}: no `jobs`", p.display());
        // `${{ }}` unquoted inside an inline map breaks the parse, because the
        // `{` opens a nested map
        for (n, l) in text.lines().enumerate() {
            let t = l.trim();
            if let (Some(mapa), Some(expr)) = (t.find(": {"), t.find("${{")) {
                // inside a quoted scalar there is an odd number of quotes
                // before the expression
                let citado = t[..expr].matches('"').count() % 2 == 1;
                assert!(
                    mapa > expr || citado,
                    "{}:{}: `${{{{ }}}}` unquoted in an inline map:\n  {t}",
                    p.display(),
                    n + 1
                );
            }
        }
        seen += 1;
    }
    assert!(seen >= 3, "only {seen} workflows were validated");
}

#[test]
fn the_generated_ci_is_valid_yaml() {
    let (yml, _, _) = axon(&["ci", "examples/payments.toml"]);
    // the real failure it had: a `: ` inside a plain multiline scalar
    for line in yml.lines() {
        let t = line.trim_start();
        if t.starts_with("- run:") || t.starts_with("run:") {
            assert!(
                !t.ends_with('\\'),
                "multiline run with no scalar block: {t}"
            );
        }
    }
    assert!(
        yml.contains("run: |"),
        "multiline commands need a scalar block"
    );
    assert!(yml.contains("id-token: write"), "no OIDC");
}

/// The only generator that hardcoded a cloud. Now the deploy comes from the
/// target, same as the infrastructure.
#[test]
fn the_ci_hardcodes_no_cloud() {
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
        // infra goes before code, with the same target
        assert!(
            yml.contains(&format!("axon infra ./ --target {target}")),
            "{target} does not apply the infra before deploying"
        );
    }
    // with no target no platform is invented
    let (without, _, _) = axon(&["ci", "examples/payments.toml"]);
    for cloud in ["gcloud", "aws ecs", "kubectl"] {
        assert!(
            !without.contains(cloud),
            "with no --target `{cloud}` showed up"
        );
    }
    assert!(
        without.contains("axon verify"),
        "with no --target the gates were lost"
    );

    // the repo layout is stated by the policy, not by axon
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
fn state_machines() {
    let (ts, _, _) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(
        ts.contains(r#"export type PaymentState = "pending" | "captured" | "failed" | "refunded""#),
        "{ts}"
    );
    assert!(ts.contains("paymentNext(state: PaymentState"));
    let (d, _, _) = axon(&["states", "examples"]);
    assert!(d.contains("pending --> captured: capture"));

    // a deadlock and a phantom trigger both have to block
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
    assert!(
        err.contains("is neither a method nor a consumed event"),
        "{err}"
    );
    assert!(err.contains("deadlock"), "{err}");
}

#[test]
fn import_asyncapi_3_and_2() {
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
        "{{amount,currency}} was not recognised as money"
    );
    // the consumer declares no fields: the schema is owned by the emitter, so
    // a consumed event's block only carries its handler
    let consumido: Vec<&str> = t3
        .lines()
        .skip_while(|l| !l.starts_with("[consumes."))
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .collect();
    assert_eq!(
        consumido,
        vec![r#"handler = "onOrderPlaced""#],
        "the import copied a foreign event's schema"
    );

    // 2.x: publish is what the app RECEIVES, subscribe what it EMITS. Inverted.
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

    // what is imported has to be immediately verifiable, and say what is missing
    let dir = std::env::temp_dir().join("axon-import");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("s.toml"), &t3).unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    // a placeholder is not a value: if this passes, the import produces lies
    assert!(
        err.contains("no `owner`"),
        "TODO was accepted as an owner:\n{err}"
    );
    assert!(err.contains("no `tier`"), "{err}");
}

/// The plugin protocol has to hold up a real generator, not just a
/// three-line check. This one is written in Go, knows nothing about axon, and
/// its output has to compile.
#[test]
fn plugin_gen_go() {
    if !has("go") {
        eprintln!("salteado: go no esta instalado");
        return;
    }
    let dir = std::env::temp_dir().join("axon-gen-go");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // the plugin is compiled and put on the PATH, as any user would
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

    // a consumed event's schema is owned by its emitter: without `peers` in
    // the protocol, the plugin could not declare this type
    assert!(code.contains("type OrderPlacedV1 struct"), "{code}");
    assert!(
        code.contains("OrderID string `json:\"orderId\"`"),
        "it does not use Go's convention"
    );
    assert!(
        code.contains("func PaymentNext(state PaymentState"),
        "no state machine"
    );
    assert!(
        code.contains("return s.outbox.Stage(ctx, e)"),
        "outbox declarado y no respetado"
    );
    assert!(code.contains("s.inbox.Once(ctx, e.ID"), "no deduplication");

    // and it compiles
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
        "the generated Go does not pass vet:\n{}",
        String::from_utf8_lossy(&vet.stderr)
    );
}

/// The example service's code has to typecheck too, not only what is
/// generated. Without this there was a gap: changing the interface axon emits
/// broke the example's implementation and the suite did not see it, because
/// Node's type stripping erases the types and nothing fails at runtime.
#[test]
fn the_example_typechecks() {
    if !has("node") || !std::path::Path::new("examples/services/node_modules").exists() {
        eprintln!("salteado: falta node o `npm i` en examples/services");
        return;
    }
    // the testkit and the contracts are regenerated so an old version on disk
    // is not what gets checked
    for (manifest, target_dir) in [
        (
            "examples/orders.toml",
            "examples/services/orders/contracts.ts",
        ),
        (
            "examples/payments.toml",
            "examples/services/payments/contracts.ts",
        ),
    ] {
        let (ts, err, ok) = axon(&["build", manifest, "examples"]);
        assert!(ok, "{err}");
        assert_eq!(
            ts.trim(),
            std::fs::read_to_string(target_dir).unwrap().trim(),
            "{target_dir} quedo desactualizado: corre axon build"
        );
    }
    let out = Command::new("npm")
        .args(["run", "typecheck"])
        .current_dir("examples/services")
        .output()
        .expect("npm run typecheck");
    assert!(
        out.status.success(),
        "the example does not typecheck:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `axon test` generates a testkit that has to compile and run against the
/// real implementation, not a skeleton with holes.
#[test]
fn the_generated_testkit_runs() {
    if !has("node") {
        eprintln!("salteado: node no esta instalado");
        return;
    }
    let pkg = std::path::Path::new("examples/services/payments");
    if !std::path::Path::new("examples/services/node_modules").exists() {
        eprintln!("salteado: falta `npm i` en examples/services");
        return;
    }

    // the committed testkit has to be up to date with the manifest
    let (kit, err, ok) = axon(&["test", "examples/payments.toml", "examples"]);
    assert!(ok, "{err}");
    let commiteado = std::fs::read_to_string(pkg.join("axon.testkit.ts")).unwrap();
    assert_eq!(
        kit.trim(),
        commiteado.trim(),
        "the committed testkit is out of date: run axon test"
    );

    let out = Command::new("node")
        .arg("--test")
        .current_dir(pkg)
        .output()
        .expect("node --test");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "the generated tests fail:\n{printed}");
    assert!(printed.contains("propagates the causal chain"), "{printed}");
    assert!(printed.contains("does not repeat the effect"), "{printed}");
    // the declared failures too: `fail`, the body on the wire and the manifest
    // are generated separately, and this is what holds them to each other
    assert!(printed.contains("declared failures"), "{printed}");
    assert!(
        printed.contains("the body on the wire carries that same code"),
        "{printed}"
    );
    assert!(printed.contains("fail 0"), "{printed}");
}

/// The gateway and the storage are not new sources of truth: they come from
/// the methods with `http` and from the `[infra.buckets]` block.
#[test]
fn the_edge_and_the_buckets_come_from_the_plan() {
    // the edge, on all four targets
    for (target, marker) in [
        ("local", "image: traefik:v3"),
        ("gcp", "google_compute_url_map"),
        ("aws", "aws_apigatewayv2_route"),
        ("k8s", "kind: HTTPRoute"),
    ] {
        let (out, _, _) = axon(&["infra", &source_for(target), "--target", target]);
        assert!(
            out.contains(marker),
            "{target} did not generate the edge ({marker})"
        );
    }
    // auth and rate limit reach the configuration, they do not stay in the manifest
    let (k, _, _) = axon(&["infra", &source_for("k8s"), "--target", "k8s"]);
    assert!(k.contains("axon.dev/auth: public"), "{k}");
    assert!(k.contains("axon.dev/rate-limit: \"60\""), "{k}");
    assert!(
        k.contains("timeouts: { request: 5s }"),
        "the edge timeout did not arrive"
    );
    let (a, _, _) = axon(&["infra", &source_for("aws"), "--target", "aws"]);
    assert!(
        a.contains("authorization_type = \"JWT\""),
        "a private route with no authorizer"
    );
    assert!(
        a.contains("authorization_type = \"NONE\""),
        "ruta publica mal marcada"
    );

    // public implies a CDN; private implies it carries none
    let (g, _, _) = axon(&["infra", &source_for("gcp"), "--target", "gcp"]);
    assert!(
        g.contains("enable_cdn  = true"),
        "a public bucket with no CDN"
    );
    assert!(
        g.contains("default_ttl = 86400"),
        "the cache_ttl did not reach the CDN"
    );
    assert!(
        g.contains("public_access_prevention    = \"enforced\""),
        "a private bucket with no lock"
    );
    assert!(g.contains("age = 2555"), "the retention did not arrive");
    assert!(
        !a.contains("cloudfront_distribution\" \"payments_receipts"),
        "a CDN over a private bucket"
    );

    // the bucket's name is a neutral template in the plan
    let (plan, _, _) = axon(&["infra", "examples", "--target", "plan"]);
    assert!(plan.contains("{project}-payments-receipts"), "{plan}");
    assert!(
        !plan.contains("var.project"),
        "the neutral plan leaked terraform syntax"
    );
    // and each target substitutes it with its own
    assert!(g.contains("${var.project}-payments-receipts"));
    assert!(
        a.contains(r#"{ name = "BUCKET_RECEIPTS", value = "${var.project}-payments-receipts" }"#),
        "the bucket name did not reach the container on aws"
    );
    assert!(k.contains("${PROJECT}-payments-receipts"));
    let (l, _, _) = axon(&["infra", "examples", "--target", "local"]);
    assert!(l.contains("BUCKET_RECEIPTS: local-payments-receipts"));
    assert!(
        l.contains("image: minio/minio:latest"),
        "local with no object storage"
    );
}

/// An exposed route with nobody deciding who may call it is an incident, not
/// a default. The edge fails closed.
#[test]
fn the_edge_fails_closed() {
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
http = "GET /v1/no-auth"
in = { a = "int" }
out = { b = "int" }
"#,
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("is exposed with no `auth`"), "{err}");
    assert!(err.contains("is public and has no `rate_limit`"), "{err}");
}

/// The security rules cite their OWASP Top 10 category, because an error that
/// does not say why it matters gets silenced with an allow.
#[test]
fn the_owasp_rules_fire() {
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
    let coloured = format!("{out}{err}");
    for regla in [
        "[A01] mal.pagar: public mutating route on a tier 0 service",
        "[A04] mal.pagar: public route with no `timeout_ms`",
        "[A09] mal.perfil: returns `email`, declared PII",
        "[A02] mal: `sk_ESTO_NO_ES_UNA_LLAVE`",
        "[A05] mal: bucket `abierto` is public and has no `retention_days`",
    ] {
        assert!(coloured.contains(regla), "falto `{regla}`:\n{coloured}");
    }

    // A05: the hardening is generated, not remembered
    let (k, _, _) = axon(&["infra", &source_for("k8s"), "--target", "k8s"]);
    for marker in [
        "runAsNonRoot: true",
        "readOnlyRootFilesystem: true",
        "capabilities: { drop: [\"ALL\"] }",
        "automountServiceAccountToken: false",
        "kind: NetworkPolicy",
    ] {
        assert!(k.contains(marker), "k8s with no `{marker}`");
    }
    // A01: with no public route there is no door to the internet
    let (g, _, _) = axon(&["infra", &source_for("gcp"), "--target", "gcp"]);
    assert!(
        g.contains("ingress  = \"INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER\""),
        "a service with no public route was left exposed"
    );
    // A08: deployed by digest, not by tag
    let (ci, _, _) = axon(&["ci", "examples/payments.toml", "--target", "gcp"]);
    assert!(
        ci.contains("@${{ steps.imagen.outputs.digest }}"),
        "deployed by a mutable tag"
    );
    // A09: the PII list and its redactor reach the code
    let (ts, _, _) = axon(&["build", "examples/orders.toml", "examples"]);
    assert!(
        ts.contains("export const piiFields = [\"customer_email\"]"),
        "{ts}"
    );
    assert!(ts.contains("export function redact"), "{ts}");
    // The same concept is declared ONCE: `customer_email` in the manifest
    // covers `customerEmail` in the contract and `customer-email` in a header.
    // It used to need declaring twice, which is absurd.
    assert!(
        ts.contains("const normalizePii"),
        "the redactor compares exact keys:\n{ts}"
    );
    assert!(ts.contains("pii.has(normalizePii(k))"), "{ts}");
    // and the normalisation reaches all three layers from a single declaration
    let (warehouse, _, _) = axon(&["analytics", "examples"]);
    assert!(
        warehouse.contains("customer_email_hash"),
        "the warehouse did not recognise the event's field:\n{warehouse}"
    );
    let (rls, _, _) = axon(&["rls", "examples"]);
    assert!(
        rls.contains(r#"'[redacted]'::text AS "customer_email""#),
        "the view did not mask the column:\n{rls}"
    );
}

/// RLS and masking are not checked by reading the SQL: they are applied to a
/// real Postgres and then looked at to see whether they isolate.
#[test]
fn the_generated_rls_really_isolates() {
    if !has("docker") {
        eprintln!("salteado: docker no esta instalado");
        return;
    }
    let (sql, err, ok) = axon(&["rls", "examples"]);
    assert!(ok, "{err}");
    assert!(
        sql.contains(r#"ALTER TABLE "order" FORCE ROW LEVEL SECURITY"#),
        "{sql}"
    );
    // `order` is a reserved word: unquoted, the SQL does not run
    assert!(
        !sql.contains("ALTER TABLE order "),
        "an unquoted identifier"
    );
    // the role is created once, not once per service
    assert_eq!(sql.matches("CREATE ROLE axon_reader").count(), 1);

    let dir = std::env::temp_dir().join("axon-rls-sql");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut coloured = String::new();
    for f in ["001_order.expand.sql", "003_tenant.expand.sql"] {
        coloured.push_str(&std::fs::read_to_string(format!("examples/sql/orders/{f}")).unwrap());
    }
    coloured.push_str(&sql);
    coloured.push_str(
        r#"
CREATE ROLE app LOGIN PASSWORD 'x';
GRANT ALL ON ALL TABLES IN SCHEMA public TO app;
GRANT SELECT ON "order_masked" TO app;
INSERT INTO "order" (id, customer_id, total_cents, status, tenant_id, customer_email)
VALUES ('11111111-1111-4111-8111-111111111111','cccccccc-0000-4000-8000-000000000001',100,'placed','aaaaaaaa-0000-4000-8000-000000000001','ana@ejemplo.mx'),
       ('22222222-2222-4222-8222-222222222222','cccccccc-0000-4000-8000-000000000002',200,'placed','bbbbbbbb-0000-4000-8000-000000000002','beto@ejemplo.mx');
SET ROLE app;
SELECT 'SIN_INQUILINO=' || count(*) FROM "order";
SET axon.tenant = 'aaaaaaaa-0000-4000-8000-000000000001';
SELECT 'INQUILINO_A=' || count(*) || ':' || min(customer_email) FROM "order";
SET axon.tenant = 'bbbbbbbb-0000-4000-8000-000000000002';
SELECT 'INQUILINO_B=' || count(*) || ':' || min(customer_email) FROM "order";
SELECT 'MASKED=' || min(customer_email) FROM "order_masked";
"#,
    );
    std::fs::write(dir.join("coloured.sql"), &coloured).unwrap();

    let name = "axon-test-rls";
    let _ = Command::new("docker").args(["rm", "-f", name]).output();
    let arranque = Command::new("docker")
        .args([
            "run",
            "-d",
            "--rm",
            "--name",
            name,
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
            "skipped: postgres could not be started: {}",
            String::from_utf8_lossy(&arranque.stderr)
        );
        return;
    }
    // it cleans up whatever happens, even if an assert blows up
    struct Limpieza(&'static str);
    impl Drop for Limpieza {
        fn drop(&mut self) {
            let _ = Command::new("docker").args(["rm", "-f", self.0]).output();
        }
    }
    let _limpieza = Limpieza(name);

    let mut listo = false;
    for _ in 0..60 {
        // pg_isready answers before the init creates the database: postgres
        // restarts halfway through its initialisation. Wait for the real database.
        let r = Command::new("docker")
            .args([
                "exec", name, "psql", "-U", "postgres", "-d", "t", "-c", "select 1",
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
            "exec", "-i", name, "psql", "-q", "-tA", "-U", "postgres", "-d", "t",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            use std::io::Write;
            c.stdin.take().unwrap().write_all(coloured.as_bytes())?;
            c.wait_with_output()
        })
        .expect("docker exec psql");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        printed.contains("SIN_INQUILINO=0"),
        "RLS does not apply with no tenant:\n{printed}"
    );
    assert!(
        printed.contains("INQUILINO_A=1:ana@ejemplo.mx"),
        "tenant A does not see its row:\n{printed}"
    );
    assert!(
        printed.contains("INQUILINO_B=1:beto@ejemplo.mx"),
        "tenant B does not see its row:\n{printed}"
    );
    assert!(
        !printed.contains("INQUILINO_A=2") && !printed.contains("INQUILINO_B=2"),
        "fuga entre inquilinos:\n{printed}"
    );
    assert!(
        printed.contains("MASKED=[redacted]"),
        "the view does not mask:\n{printed}"
    );

    // How the tenant is pinned matters as much as the policy, and this
    // measures it instead of inferring it. The result is stronger than "use
    // SET LOCAL": a single session `SET` poisons the connection for every
    // later SET LOCAL, because SET LOCAL reverts to the SESSION's value and
    // not to nothing.
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
            "exec", "-i", name, "psql", "-q", "-tA", "-U", "postgres", "-d", "t",
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
    // on a clean connection, SET LOCAL does not survive the commit
    assert!(
        s2.contains("LIMPIA=SIN_FIJAR"),
        "SET LOCAL survived the commit:\n{s2}"
    );
    // but after a session `SET`, it reverts to THAT value: the leak persists
    assert!(
        s2.contains("ENVENENADA=bbbbbbbb-0000-4000-8000-000000000002"),
        "the measured behaviour changed; review the RLS prescription:\n{s2}"
    );
    assert!(s2.contains("TRAS_RESET=SIN_FIJAR"), "{s2}");

    // and the generated prescription says exactly that
    assert!(
        sql.contains("NEVER a session `SET`"),
        "the migration does not prescribe how to pin the tenant"
    );
    assert!(
        sql.contains("NULLIF(current_setting('axon.tenant', true), '')::uuid"),
        "{sql}"
    );
}

/// The gravest gap the tool had: a field could be changed on an already
/// published version and `verify` came out clean.
#[test]
fn a_published_version_is_immutable() {
    let base = std::env::temp_dir().join("axon-baseline");

    // prepares a copy of the examples with their baseline, with no migrations
    // (only contracts are tested here)
    let preparar = |suffix: &str| -> std::path::PathBuf {
        let dir = base.join(suffix);
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

    // the starting point is clean
    let dir = preparar("limpio");
    let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "{err}");
    assert!(out.contains("0 errors"), "{out}");
    assert!(
        !out.contains("not recorded"),
        "the freshly taken baseline already has gaps"
    );

    let change = |suffix: &str, file: &str, from: &str, to: &str| -> String {
        let dir = preparar(suffix);
        let p = dir.join(file);
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains(from), "the fixture does not contain `{from}`");
        std::fs::write(&p, t.replace(from, to)).unwrap();
        let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        assert!(!ok, "the change `{from}` -> `{to}` passed clean");
        err
    };

    // a changed type
    let err = change(
        "tipo",
        "orders.toml",
        "total = \"money\"",
        "total = \"int\"",
    );
    assert!(
        err.contains("changed from `money` to `int` in a published version"),
        "{err}"
    );
    assert!(
        err.contains("publish `order.placed@v2`"),
        "it does not say what to do:\n{err}"
    );

    // an added field: in axon every field is mandatory, so it breaks all the same
    let err = change(
        "agregado",
        "orders.toml",
        "\ntotal = \"money\"\n",
        "\ntotal = \"money\"\nchannel = \"string\"\n",
    );
    assert!(
        err.contains("new field `channel` in a published version"),
        "{err}"
    );

    // a moved route
    let err = change(
        "ruta",
        "orders.toml",
        "http = \"POST /v1/tenants/{tenantId}/orders\"",
        "http = \"POST /v1/pedidos\"",
    );
    assert!(
        err.contains("the route changed from `POST /v1/tenants/{tenantId}/orders`"),
        "{err}"
    );
    assert!(
        !err.contains("Some("),
        "the message leaks Option's Debug:\n{err}"
    );

    // retiring a published version
    let err = change(
        "retiro",
        "orders.toml",
        "[emits.\"order.placed@v1\"]",
        "[emits.\"order.placed@v2\"]",
    );
    assert!(
        err.contains("it was published by orders and nobody emits it any more"),
        "{err}"
    );

    // the escape hatch: retiring it from the baseline too, visible in the PR
    let dir = preparar("retiro_deliberado");
    for (f, from, to) in [
        (
            "orders.toml",
            "[emits.\"order.placed@v1\"]",
            "[emits.\"order.placed@v2\"]",
        ),
        // The metrics read that event, so retiring it moves them too. That is
        // the point of the rule, not a nuisance: a metric left reading a retired
        // event answers zero forever, and zero reads like nothing was sold.
        (
            "orders.toml",
            "on     = [\"order.placed@v1\"]",
            "on     = [\"order.placed@v2\"]",
        ),
        ("payments.toml", "order.placed@v1", "order.placed@v2"),
    ] {
        let p = dir.join(f);
        let t = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, t.replace(from, to)).unwrap();
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
    assert!(ok, "a deliberate retirement should not block:\n{err}");

    // a new unregistered contract is not protected, and that has to be said
    let dir = preparar("nuevo");
    let p = dir.join("orders.toml");
    let t = std::fs::read_to_string(&p).unwrap();
    std::fs::write(
        &p,
        format!("{t}\n[emits.\"order.cancelled@v1\"]\norderId = \"uuid\"\n"),
    )
    .unwrap();
    let (out, _, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "a new contract is not an error");
    assert!(out.contains("not recorded"), "{out}");

    // and with no baseline, `verify` has to say it cannot see this
    let dir = preparar("sin_baseline");
    std::fs::remove_file(dir.join("axon.baseline.json")).unwrap();
    let (out, _, _) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(out.contains("no axon.baseline.json"), "{out}");
}

/// The declared resilience has to be executed, not just validated: it was the
/// only promise in the manifest that did not reach the code.
#[test]
fn the_declared_policy_is_executed() {
    if !has("node") || !std::path::Path::new("examples/services/node_modules").exists() {
        eprintln!("salteado: falta node o `npm i` en examples/services");
        return;
    }
    let (ts, err, ok) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(ok, "{err}");
    // the manifest's numbers reach the code literally
    assert!(
        ts.contains(
            r#"withPolicy("orders.getOrder", { timeoutMs: 1000, retries: 3, breaker: true }"#
        ),
        "the policy did not come from the manifest:\n{ts}"
    );
    // retrying with no idempotency key duplicates the effect on the other side
    assert!(ts.contains(r#"headers(e, true)"#));
    // CAP: the declared side decides the isolation
    assert!(
        ts.contains(r#"export const isolationLevel = "SERIALIZABLE""#),
        "{ts}"
    );
    let (o, _, _) = axon(&["build", "examples/orders.toml", "examples"]);
    assert!(
        o.contains(r#"export const isolationLevel = "READ COMMITTED""#),
        "{o}"
    );
    assert!(o.contains("export const maxStalenessMs = 3000"), "{o}");
    // `degrade` forces passing the degraded path; `reject` does not accept it
    assert!(
        o.contains("fallback: () => Promise<PaymentsCapturePaymentOut>"),
        "declaring degrade did not force a fallback:\n{o}"
    );
    assert!(
        !ts.contains("fallback: () =>"),
        "a `reject` service does not degrade"
    );

    // and the policy behaves: it is executed against the generated code
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
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the generated policy does not behave:\n{printed}"
    );
    assert!(printed.contains("pass 4"), "{printed}");
}

/// CAP: the partition is not a choice, what to do while it lasts is.
#[test]
fn the_cap_side_is_verified() {
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
    assert!(err.contains("with no `max_staleness_ms`"), "{err}");

    // the theorem's contradiction: CP serving something stale
    std::fs::write(
        dir.join("c.toml"),
        "service = \"c\"\nowner = \"x\"\ntier = \"1\"\n[cap]\nconsistency = \"strong\"\non_partition = \"degrade\"\n",
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok);
    assert!(err.contains("contradicts itself"), "{err}");

    // undeclared, the pair that fails closed is assumed, and it says so
    std::fs::write(
        dir.join("c.toml"),
        "service = \"c\"\nowner = \"x\"\ntier = \"1\"\n",
    )
    .unwrap();
    let (out, _, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok);
    assert!(out.contains("no `[cap]`; assumed CP"), "{out}");

    // a synchronous path's guarantee is the weakest link's
    let (out, _, _) = axon(&["verify", "examples"]);
    assert!(
        out.contains("is `strong` and calls orders, which is `eventual`"),
        "{out}"
    );
}

/// OpenTelemetry: axon ships no SDK and invents no format. The envelope
/// already propagates `traceparent`, which is the W3C context OTel uses, so it
/// only brings up the backend locally and puts the standard variables on all
/// four targets —the destination changes, the attributes do not.
#[test]
fn otel_on_all_four_targets() {
    let expected = [
        "OTEL_SERVICE_NAME",
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "OTEL_EXPORTER_OTLP_PROTOCOL",
        "OTEL_RESOURCE_ATTRIBUTES",
        "OTEL_TRACES_SAMPLER",
    ];
    for target in ["local", "gcp", "aws", "k8s"] {
        let (out, err, ok) = axon(&["infra", &source_for(target), "--target", target]);
        assert!(ok, "{target}: {err}");
        for v in expected {
            assert!(out.contains(v), "{target} does not inject {v}");
        }
        // the resource attributes come from the manifest, not from a convention
        assert!(
            out.contains("axon.owner=payments-team") && out.contains("axon.tier=0"),
            "{target}: the attributes do not come from the manifest"
        );
    }

    // the destination is the only thing that changes between targets
    let (l, _, _) = axon(&["infra", "examples", "--target", "local"]);
    assert!(l.contains("http://trace:4318"));
    assert!(
        l.contains("image: jaegertracing/all-in-one"),
        "local with no trace backend"
    );
    for (target, endpoint) in [
        ("gcp", "${var.otlp_endpoint}"),
        ("aws", "${var.otlp_endpoint}"),
        ("k8s", "${OTLP_ENDPOINT}"),
    ] {
        let (o, _, _) = axon(&["infra", &source_for(target), "--target", target]);
        assert!(
            o.contains(endpoint),
            "{target} with no configurable OTLP destination"
        );
    }

    // the sampling comes from the tier: a tier 0 is traced whole, because when
    // it goes down the missing trace is exactly the one that was needed
    let (g, _, _) = axon(&["infra", &source_for("gcp"), "--target", "gcp"]);
    assert!(
        g.contains("parentbased_always_on"),
        "tier 0 with no full sampling"
    );
    assert!(
        g.contains("parentbased_traceidratio"),
        "tier 1 with no partial sampling"
    );
    // but locally everything is traced: dropping 90% while you debug is no use
    assert!(
        !l.contains("parentbased_traceidratio"),
        "local muestrea parcialmente"
    );

    // the traceparent's flags are inherited, not invented: declaring
    // "sampled" over a trace that is not breaks the tree into fragments
    let (ts, _, _) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(ts.contains(r#"const flags = parts?.[3] ?? "01""#), "{ts}");
    assert!(!ts.contains("${hex(8)}-01`"), "the envelope pins the flags");
}

/// pg_anon's dictionary: every `pii` field gets a rule, and every rule really
/// applies to a column of its type. A function that does not exist —or a
/// missing cast— makes the dump fail halfway through.
#[test]
fn the_pg_anon_dictionary_works() {
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

    let (dict, err, ok) = axon(&["rls", dir.to_str().unwrap(), "--target", "pg_anon"]);
    assert!(ok, "{err}");
    // coverage: no declared field is left without a rule
    for field in [
        "id",
        "correo",
        "nacimiento",
        "ingreso",
        "preferencias",
        "verificado",
    ] {
        assert!(
            dict.contains(&format!("\"{field}\":")),
            "no rule for {field}:\n{dict}"
        );
    }
    // md5(uuid) does not exist: the inner cast is mandatory
    assert!(dict.contains(r#"md5(\"id\"::text)::uuid"#), "{dict}");
    // the rule comes from the type, not from the name: `correo` carries no
    // "mail" and still gets the text rule
    assert!(dict.contains("anon_funcs.digest(\\\"correo\\\""), "{dict}");
    // the framework's own tables are excluded: the outbox carries payloads
    let (dic2, _, _) = axon(&["rls", "examples", "--target", "pg_anon"]);
    assert!(
        dic2.contains("\"table\": \"outbox\"") && dic2.contains("dictionary_exclude"),
        "{dic2}"
    );

    if !has("docker") {
        eprintln!("skipping the rest: docker is not installed");
        return;
    }

    // every rule, applied to a column of its type
    let name = "axon-test-pganon";
    let _ = Command::new("docker").args(["rm", "-f", name]).output();
    let arranque = Command::new("docker")
        .args([
            "run",
            "-d",
            "--rm",
            "--name",
            name,
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
    let _l = Limpieza(name);
    let mut listo = false;
    for _ in 0..60 {
        if Command::new("docker")
            .args([
                "exec", name, "psql", "-U", "postgres", "-d", "t", "-c", "select 1",
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
         -- a double of anon_funcs.digest, only to check the SHAPE of the call:\n\
         -- pg_anon installs the real function at the destination.\n\
         CREATE SCHEMA anon_funcs;\n\
         CREATE FUNCTION anon_funcs.digest(t text, salt text, algo text) RETURNS text\n  \
           AS $$ SELECT encode(sha256((t || salt)::bytea), 'hex') $$ LANGUAGE sql IMMUTABLE;\n",
    );
    // every dictionary rule, as-is, against its column
    for line in dict.lines() {
        let l = line.trim();
        if !l.starts_with('"') || !l.contains("\": \"") {
            continue;
        }
        let Some((field, resto)) = l.split_once("\": \"") else {
            continue;
        };
        let field = field.trim_start_matches('"');
        if ["schema", "table"].contains(&field) {
            continue;
        }
        // the value comes escaped for Python: here it is unescaped for SQL
        let regla = resto
            .trim_end_matches(&[',', '"'][..])
            .replace("\\\"", "\"");
        sql.push_str(&format!("SELECT {regla} FROM persona;\n"));
    }

    let out = Command::new("docker")
        .args([
            "exec",
            "-i",
            name,
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
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "a generated rule does not run on Postgres:\n{printed}"
    );
    // the original data survives no rule
    assert!(
        !printed.contains("ana@gmail.com"),
        "the address was not masked:\n{printed}"
    );
    assert!(
        printed.contains("2026-01-01"),
        "the date was not truncated:\n{printed}"
    );
}

/// Database scaling: arithmetic over what is declared. Connection exhaustion
/// does not show up with one instance; it shows up the day it scales.
#[test]
fn the_database_scaling_is_verified() {
    let dir = std::env::temp_dir().join("axon-escala");
    let escribir = |cuerpo: &str| {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("s.toml"), cuerpo).unwrap();
        let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        (format!("{out}{err}"), ok)
    };

    // 20 x 10 instances = 200 connections against a ceiling of 100
    let (msg, ok) = escribir(
        "service = \"s\"\nowner = \"x\"\ntier = \"2\"\n[infra]\nstate = \"postgres\"\n\
         pool_size = 20\nmax_connections = 100\nmax_instances = 10\n",
    );
    assert!(!ok);
    assert!(msg.contains("20 connections x 10 instances = 200"), "{msg}");
    assert!(msg.contains("exceeds the limit of 100"), "{msg}");

    // a replica lags: reading from it and promising strong consistency is the
    // theorem's contradiction written in two places
    let (msg, ok) = escribir(
        "service = \"s\"\nowner = \"x\"\ntier = \"2\"\n[cap]\nconsistency = \"strong\"\n\
         [infra]\nstate = \"postgres\"\nread_replicas = 2\n",
    );
    assert!(!ok);
    assert!(msg.contains("reads from 2 replicas and declares"), "{msg}");

    // high availability is not a fallback: the standby replicates the DROP TABLE
    let (msg, ok) = escribir(
        "service = \"s\"\nowner = \"x\"\ntier = \"0\"\n[infra]\nstate = \"postgres\"\nha = true\n",
    );
    assert!(!ok);
    assert!(msg.contains("with no `backup_retention_days`"), "{msg}");
    assert!(msg.contains("is not a backup"), "{msg}");

    // a tier 0 with no failover is not a tier 0
    let (msg, ok) = escribir(
        "service = \"s\"\nowner = \"x\"\ntier = \"0\"\n[infra]\nstate = \"postgres\"\n\
         backup_retention_days = 30\n",
    );
    assert!(!ok);
    assert!(msg.contains("with no `ha = true`"), "{msg}");

    // and the resources: standby, backups and replicas come from the manifest
    let (g, _, _) = axon(&["infra", &source_for("gcp"), "--target", "gcp"]);
    assert!(
        g.contains("availability_type = \"REGIONAL\""),
        "payments is tier 0: the standby is missing"
    );
    assert!(g.contains("retained_backups = 30"), "{g}");
    assert!(g.contains("point_in_time_recovery_enabled = true"), "{g}");
    assert!(
        g.contains("master_instance_name = google_sql_database_instance.orders.name"),
        "no replicas"
    );
    // database-per-service is one INSTANCE per service
    assert!(
        g.contains("resource \"google_sql_database_instance\" \"payments\""),
        "{g}"
    );
    assert!(
        !g.contains("var.sql_instance"),
        "the databases still share an instance"
    );
    let (a, _, _) = axon(&["infra", &source_for("aws"), "--target", "aws"]);
    assert!(a.contains("multi_az                = true"), "{a}");
    assert!(a.contains("backup_retention_period = 30"), "{a}");
    assert!(
        a.contains("replicate_source_db = aws_db_instance.orders.identifier"),
        "{a}"
    );
}

/// The load test comes from the manifest, and its verdict compares the
/// measured against the declared. A declared number nobody measures is an opinion.
#[test]
fn the_load_test_comes_from_the_manifest() {
    let (js, err, ok) = axon(&["load", "examples/orders.toml"]);
    assert!(ok, "{err}");
    // the rate is the declared rate_limit, not a number picked by eye
    assert!(
        js.contains("rate: 60,              // declared in rate_limit"),
        "{js}"
    );
    // the threshold is the declared timeout
    assert!(
        js.contains(r#""http_req_duration{scenario:placeOrder}": ["p(95)<5000"]"#),
        "{js}"
    );
    assert!(
        js.contains(r#""http_req_duration{scenario:getOrder}": ["p(95)<2000"]"#),
        "{js}"
    );
    // a route with a parameter is tested with a made-up id: a 404 there is not
    // a service failure, so the threshold goes on the check
    assert!(
        js.contains(r#""checks{scenario:getOrder}": ["rate>0.99"]"#),
        "{js}"
    );
    assert!(js.contains("r.status === 404"), "{js}");
    // the ceiling the declared pool imposes is written down
    assert!(js.contains("4 connections x 10 instances = 40"), "{js}");

    // and the verdict: `true` on a k6 threshold means BREACHED
    let dir = std::env::temp_dir().join("axon-carga");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let good = dir.join("bien.json");
    std::fs::write(
        &good,
        r#"{"metrics":{"http_req_duration{scenario:placeOrder}":{"p(95)":23.8,"thresholds":{"p(95)<5000":false}},"http_reqs":{"count":41,"rate":2.05}}}"#,
    )
    .unwrap();
    let (out, err, ok) = axon(&[
        "load",
        "examples/orders.toml",
        "--check",
        good.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    assert!(out.contains("0 thresholds breached"), "{out}");
    assert!(out.contains("41 requests measured"), "{out}");

    let bad = dir.join("mal.json");
    std::fs::write(
        &bad,
        r#"{"metrics":{"http_req_duration{scenario:placeOrder}":{"p(95)":9000,"thresholds":{"p(95)<5000":true}}}}"#,
    )
    .unwrap();
    let (_, err, ok) = axon(&[
        "load",
        "examples/orders.toml",
        "--check",
        bad.to_str().unwrap(),
    ]);
    assert!(!ok, "a breached threshold has to fail");
    assert!(err.contains("breached `p(95)<5000`"), "{err}");

    // a summary with no thresholds is not a verdict, and saying so beats
    // taking what was not measured as fine
    let empty = dir.join("vacio.json");
    std::fs::write(&empty, r#"{"metrics":{"http_reqs":{"count":1,"rate":1}}}"#).unwrap();
    let (_, err, ok) = axon(&[
        "load",
        "examples/orders.toml",
        "--check",
        empty.to_str().unwrap(),
    ]);
    assert!(!ok);
    assert!(err.contains("carries no thresholds"), "{err}");
}

/// A declared route nobody serves returns a 404 in production and shows up in
/// no test. The generated code publishes the list so startup can refuse.
#[test]
fn the_declared_routes_reach_the_code() {
    let (ts, _, _) = axon(&["build", "examples/orders.toml", "examples"]);
    assert!(
        ts.contains(
            r#"export const httpRoutes = ["POST /v1/tenants/{tenantId}/orders", "GET /v1/tenants/{tenantId}/orders/{orderId}", "GET /v2/tenants/{tenantId}/orders/{orderId}"]"#
        ),
        "{ts}"
    );
}

/// Feature flags: what declaring them adds is not the SDK, but that the
/// compiler enforces what nobody enforces.
#[test]
fn the_flags_are_verified() {
    let dir = std::env::temp_dir().join("axon-flags");
    let probar = |cuerpo: &str| {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.toml"), cuerpo).unwrap();
        let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        (format!("{out}{err}"), ok)
    };
    let base = "service = \"f\"\nowner = \"e\"\ntier = \"2\"\n";

    // a flag with no death date does not die
    let (msg, ok) = probar(&format!("{base}[flags.eterno]\nowner = \"e\"\n"));
    assert!(!ok);
    assert!(msg.contains("no `expires`"), "{msg}");

    // an expired one is not ignored: it gets cleaned up or renewed
    let (msg, ok) = probar(&format!(
        "{base}[flags.viejo]\nowner = \"e\"\nexpires = \"2024-01-15\"\n"
    ));
    assert!(!ok);
    assert!(msg.contains("expired on 2024-01-15"), "{msg}");

    // with no owner, nobody turns it off
    let (msg, ok) = probar(&format!(
        "{base}[flags.huerfano]\nexpires = \"2099-01-01\"\n"
    ));
    assert!(!ok);
    assert!(msg.contains("flag with no `owner`"), "{msg}");

    // a per-request rollout leaves the same entity half migrated
    let (msg, ok) = probar(&format!(
        "{base}[flags.parcial]\nowner = \"e\"\nexpires = \"2099-01-01\"\nrollout = 25\n"
    ));
    assert!(!ok);
    assert!(msg.contains("rollout at 25% with no `sticky_by`"), "{msg}");

    // a kill switch goes off whole: the error is its own, not the sticky one's
    let (msg, ok) = probar(&format!(
        "{base}[flags.cortar]\nowner = \"e\"\nkill_switch = true\nrollout = 50\n"
    ));
    assert!(!ok);
    assert!(msg.contains("`kill_switch` with `rollout`"), "{msg}");

    // pinning by a field the service does not receive pins nothing
    let (msg, ok) = probar(&format!(
        "{base}[flags.malo]\nowner = \"e\"\nexpires = \"2099-01-01\"\nrollout = 50\n\
         sticky_by = \"no_existe\"\n"
    ));
    assert!(!ok);
    assert!(msg.contains("appears in no contract"), "{msg}");

    // a kill switch with no expires is the right thing, and it does not complain
    let (_, ok) = probar(&format!(
        "{base}[flags.corte]\nowner = \"e\"\nkill_switch = true\n"
    ));
    assert!(ok, "a legitimate kill switch should not fail");

    // the code: the accessor requires the field it is pinned by
    let (ts, _, _) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(
        ts.contains(
            "export const flagChargeV2 = (flags: Flags, tenant_id: string): Promise<boolean> =>"
        ),
        "{ts}"
    );
    assert!(
        ts.contains(
            r#"flags.evaluate("charge_v2", false, { targetingKey: tenant_id, tenant_id })"#
        ),
        "{ts}"
    );
    assert!(
        ts.contains(r#"export const declaredFlags = ["charge_v2""#),
        "{ts}"
    );

    // OpenFeature's four types: a flag is not just a boolean, and a config
    // rollout —a limit, a provider— needs the others
    assert!(
        ts.contains(
            "export const flagChargeProvider = (flags: Flags, tenant_id: string): Promise<string> =>"
        ),
        "no typed accessor for a string flag:\n{ts}"
    );
    assert!(
        ts.contains(r#"flags.evaluate("charge_provider", "stripe", "#),
        "{ts}"
    );
    assert!(
        ts.contains("export const flagRetryLimit = (flags: Flags): Promise<number> =>"),
        "no typed accessor for a numeric flag:\n{ts}"
    );
    // the interface covers the standard's four types
    assert!(
        ts.contains("evaluate<T extends boolean | string | number | object>"),
        "{ts}"
    );

    // and flagd's config: the rollout is expressed with its `fractional`
    let (cfg, _, _) = axon(&["flags", "examples"]);
    let v: serde_json::Value = serde_json::from_str(&cfg).expect("flagd json");
    let f = &v["flags"]["charge_v2"];
    assert_eq!(f["defaultVariant"], "off");
    assert_eq!(f["targeting"]["fractional"][0]["var"], "tenant_id");
    assert_eq!(f["targeting"]["fractional"][1][1], 10);
    assert_eq!(f["targeting"]["fractional"][2][1], 90);
    // a kill switch carries no targeting
    assert!(v["flags"]["stripe_kill"]["targeting"].is_null());

    // and the declared variants reach flagd as-is, not a fixed on/off
    let p = &v["flags"]["charge_provider"];
    assert_eq!(p["variants"]["stripe"], "stripe");
    assert_eq!(p["variants"]["adyen"], "adyen");
    assert_eq!(p["defaultVariant"], "stripe");
    // the rollout splits between the default variant and the other one
    assert_eq!(p["targeting"]["fractional"][1][0], "adyen");
    assert_eq!(p["targeting"]["fractional"][1][1], 20);
    assert_eq!(p["targeting"]["fractional"][2][0], "stripe");
    assert_eq!(v["flags"]["retry_limit"]["variants"]["normal"], 3);

    // a non-existent default variant makes evaluation always fall back to the
    // code's value, and the flag stops working in silence
    let (msg, ok) = probar(&format!(
        "{base}[flags.raro]\nowner = \"e\"\nkill_switch = true\n\
         default_variant = \"no_existe\"\nvariants = {{ a = \"x\" }}\n"
    ));
    assert!(!ok);
    assert!(msg.contains("is not in `variants`"), "{msg}");

    // OpenFeature resolves one type per flag, not one per variant
    let (msg, ok) = probar(&format!(
        "{base}[flags.mezcla]\nowner = \"e\"\nkill_switch = true\n\
         default_variant = \"a\"\nvariants = {{ a = \"x\", b = 2 }}\n"
    ));
    assert!(!ok);
    assert!(msg.contains("mix types"), "{msg}");
}

/// `axon cap` does not repeat what `verify` blocks: it explains the
/// consequences. There are combinations that are not an error and still change
/// what the service can promise, and that is worth writing down before an incident.
#[test]
fn the_cap_report_reconciles_the_patterns() {
    let (out, err, ok) = axon(&["cap", "examples"]);
    assert!(ok, "{err}");
    // contradicts: verify already blocks it, and here the why is explained
    assert!(out.contains("contradicts"), "{out}");
    assert!(
        out.contains("the path's guarantee is the weaker one"),
        "{out}"
    );
    // costs: a compensation is eventual consistency by construction
    assert!(
        out.contains("your own state is CP, the FLOW is not"),
        "{out}"
    );
    // implies: the outbox does not break your guarantee, it breaks the flow's
    assert!(out.contains("consumers see it late"), "{out}");
    // and the standby is the only thing giving availability at no cost in consistency
    assert!(out.contains("at no cost in the C"), "{out}");
    assert!(out.contains("[CP]") && out.contains("[AP]"), "{out}");

    // the per-service filter, with the analysis still looking at all of them:
    // without `orders` loaded there would be no way to know the dependency is AP
    let (only, _, _) = axon(&["cap", "examples", "-s", "payments"]);
    assert!(only.contains("payments"), "{only}");
    assert!(
        !only.contains("\norders "),
        "the filter did not narrow:\n{only}"
    );
    assert!(only.contains("`orders`, which is AP, is called"), "{only}");

    let (nothing, _, _) = axon(&["cap", "examples", "-s", "inexistente"]);
    assert!(nothing.contains("no service by that name"), "{nothing}");
}

/// Colours: blue informs, yellow warns, red blocks. And they turn themselves
/// off when the output is not a terminal, because there the sequences are junk
/// that dirties a diff or a CI log.
#[test]
fn the_colours_respect_the_destination() {
    // the suite captures the output, so it is never a terminal
    let (out, err, _) = axon(&["verify", "examples"]);
    let coloured = format!("{out}{err}");
    assert!(
        !coloured.contains('\x1b'),
        "it coloured an output that is not a terminal"
    );

    // and with CLICOLOR_FORCE it does colour
    let forzado = Command::new(env!("CARGO_BIN_EXE_axon"))
        .args(["verify", "examples"])
        .env("CLICOLOR_FORCE", "1")
        .output()
        .expect("axon");
    let coloured = format!(
        "{}{}",
        String::from_utf8_lossy(&forzado.stdout),
        String::from_utf8_lossy(&forzado.stderr)
    );
    assert!(
        coloured.contains("\x1b[1;33m"),
        "it did not colour with CLICOLOR_FORCE"
    );

    // NO_COLOR wins over the forcing, which is the convention
    let plain = Command::new(env!("CARGO_BIN_EXE_axon"))
        .args(["verify", "examples"])
        .env("CLICOLOR_FORCE", "1")
        .env("NO_COLOR", "1")
        .output()
        .expect("axon");
    let coloured = format!(
        "{}{}",
        String::from_utf8_lossy(&plain.stdout),
        String::from_utf8_lossy(&plain.stderr)
    );
    assert!(!coloured.contains('\x1b'), "NO_COLOR was not honoured");
}

/// The book quotes axon's output, and nothing checked that quote. That is how it went
/// stale: for a while the pages showed messages in Spanish that the tool had stopped
/// printing, right next to manifest examples that CI does check. Both halves of a page
/// have to be checkable, or the uncheckable half is the one that lies.
///
/// So every quoted message is looked for IN THE SOURCE that prints it: four consecutive
/// words of the quote have to appear in some message of `src/`, compared with
/// punctuation and case removed, because a message is wrapped in the source and wrapped
/// differently in the book. A paraphrased quote —or one in a language the tool no longer
/// speaks— has no window that matches.
#[test]
fn the_docs_quote_output_the_tool_really_prints() {
    // the labels `axon` prints, and the ones it printed BEFORE the migration: quoting
    // those is quoting a tool that no longer exists
    const LABELS: [&str; 6] = ["error", "warn", "near", "fail", "info", "ok"];
    const GONE: [&str; 4] = ["aviso", "falla", "cerca", "correcto"];

    // punctuation and case out: the same message is wrapped one way in the source and
    // another in the book, and it carries interpolated values in between
    let plain = |t: &str| -> String {
        let mut o = String::from(" ");
        let mut space = true;
        for c in t.chars() {
            if c.is_ascii_alphabetic() {
                o.push(c.to_ascii_lowercase());
                space = false;
            } else if !space {
                o.push(' ');
                space = true;
            }
        }
        o
    };

    let mut corpus = String::new();
    for entry in std::fs::read_dir("src").unwrap().flatten() {
        if entry.path().extension().is_some_and(|e| e == "rs") {
            corpus.push_str(&plain(&std::fs::read_to_string(entry.path()).unwrap()));
        }
    }

    let mut quoted = 0;
    for entry in std::fs::read_dir("docs/src").unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut inside = false;
        // the message being accumulated: its line, and its text so far
        let mut current: Option<(usize, String)> = None;
        let check = |line: usize, msg: &str| {
            let flat = plain(msg);
            let words: Vec<&str> = flat.split_whitespace().collect();
            // four words, or the whole message when it is shorter: the summary line
            // —`4 services, 3 errors, 1 warnings`— is three words once the numbers go
            let n = words.len().min(4);
            assert!(n >= 2, "{name}:{line}: nothing to compare in `{msg}`");
            let found = words
                .windows(n)
                .any(|w| corpus.contains(&format!(" {} ", w.join(" "))));
            assert!(
                found,
                "{name}:{line}: this quoted message is in no message of `src/`. \
                 Either axon stopped printing it or it was paraphrased:\n  {msg}"
            );
        };
        for (n, line) in text.lines().enumerate() {
            let n = n + 1;
            if line.starts_with("```") {
                if let Some((l, m)) = current.take() {
                    check(l, &m);
                    quoted += 1;
                }
                inside = line.starts_with("```console");
                continue;
            }
            if !inside {
                continue;
            }
            for gone in GONE {
                assert!(
                    !line.starts_with(&format!("{gone} ")),
                    "{name}:{n}: quotes `{gone}`, a label axon does not print any more:\n  {line}"
                );
            }
            if LABELS.iter().any(|l| line.starts_with(&format!("{l} "))) {
                if let Some((l, m)) = current.take() {
                    check(l, &m);
                    quoted += 1;
                }
                current = Some((n, line.split_once(' ').unwrap().1.to_string()));
            } else if line.starts_with("       ") && current.is_some() {
                // a continuation line of the same message
                let (l, mut m) = current.take().unwrap();
                m.push(' ');
                m.push_str(line.trim());
                current = Some((l, m));
            } else if let Some((l, m)) = current.take() {
                check(l, &m);
                quoted += 1;
            }
        }
        if let Some((l, m)) = current {
            check(l, &m);
            quoted += 1;
        }
    }
    assert!(
        quoted >= 15,
        "only {quoted} quoted messages were checked; the scan stopped seeing them"
    );
    eprintln!("{quoted} quoted messages checked against `src/`");
}

/// The documentation examples are not text: every ```toml block in
/// `docs/src/` is run through `axon verify`. An example that does not validate
/// breaks CI, so the documentation cannot go stale in silence —which is
/// exactly how all documentation goes.
#[test]
fn the_documentation_examples_validate() {
    let dir = std::env::temp_dir().join("axon-docs");
    let mut checked = 0;
    let mut pages = 0;

    for e in std::fs::read_dir("docs/src").expect("docs/src") {
        let pagina = e.unwrap().path();
        if pagina.extension().is_none_or(|x| x != "md") {
            continue;
        }
        pages += 1;
        let text = std::fs::read_to_string(&pagina).unwrap();
        let name = pagina.file_name().unwrap().to_string_lossy().to_string();

        // the ```toml blocks, in order, with their line number for the message
        let mut dentro = false;
        let mut inicio = 0usize;
        let mut bloque = String::new();
        let mut bloques: Vec<(usize, String)> = Vec::new();
        for (n, l) in text.lines().enumerate() {
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

        for (line, cuerpo) in bloques {
            // A block with no `service` is a fragment —a policy, a piece of
            // `[infra]`— and not a manifest: it gets a minimal header so it
            // can be parsed all the same.
            let manifest = if cuerpo.contains("service = ") {
                cuerpo.clone()
            } else if cuerpo.trim_start().starts_with('[') || cuerpo.contains(" = ") {
                format!("service = \"doc\"\nowner = \"docs\"\ntier = \"2\"\n{cuerpo}")
            } else {
                continue;
            };

            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("doc.toml"), &manifest).unwrap();
            let (out, err, _) = axon(&["verify", dir.to_str().unwrap()]);
            let printed = format!("{out}{err}");

            // What is checked is that the TOML is valid and that the manifest
            // can be loaded: an example may fail rules on purpose, because
            // many of them illustrate exactly an error.
            assert!(
                !printed.contains("TOML parse error") && !printed.contains("falta `service`"),
                "{name}:{line}: the example is not a valid manifest:\n{printed}\n---\n{manifest}"
            );
            checked += 1;
        }
    }
    assert!(pages >= 10, "only {pages} pages of docs/src were read");
    assert!(checked >= 10, "only {checked} examples were checked");
    eprintln!("{checked} manifest examples across {pages} pages");
}

/// The architecture page draws the compiler's own modules and claims, target by
/// target, which resources each render emits. Neither is derivable from a
/// manifest —it is a diagram of axon itself— so it is the one page that can go
/// stale without anything noticing.
///
/// So it gets checked against the code it describes: every module in `src/` is in
/// the map and every module the map names exists, and every provider resource the
/// page names is really emitted by that target.
#[test]
fn the_architecture_page_matches_the_code_it_describes() {
    let page = std::fs::read_to_string("docs/src/architecture.md").unwrap();

    // --- the module map ---------------------------------------------------
    let mut modules = Vec::new();
    for entry in std::fs::read_dir("src").unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "rs") {
            modules.push(path.file_name().unwrap().to_string_lossy().to_string());
        }
    }
    assert!(modules.len() >= 12, "only {} modules found", modules.len());
    for m in &modules {
        assert!(
            page.contains(m),
            "`{m}` is not in the architecture page's map. A module that nobody \
             drew is a module nobody knows exists"
        );
    }
    // and the other way round: a module the page names has to exist, or the
    // drawing describes a compiler that is not this one
    let mut named = 0;
    let bytes: Vec<char> = page.chars().collect();
    for (i, w) in bytes.windows(3).enumerate() {
        if w != ['.', 'r', 's'] {
            continue;
        }
        let start = bytes[..i]
            .iter()
            .rposition(|c| !(c.is_ascii_lowercase() || *c == '_'))
            .map_or(0, |p| p + 1);
        let name: String = bytes[start..i + 3].iter().collect();
        if name.len() > 3 {
            assert!(
                std::path::Path::new("src").join(&name).exists(),
                "the page names `{name}` and `src/{name}` does not exist"
            );
            named += 1;
        }
    }
    assert!(named >= 12, "only {named} modules named in the page");

    // --- what each target really emits ------------------------------------
    // The page's table says, for instance, that `[infra] state` becomes a
    // `google_sql_database_instance` on gcp and an `aws_db_parameter_group` on
    // aws. Those are claims about the renderer, and the renderer can answer.
    for (target, prefix) in [("gcp", "google_"), ("aws", "aws_")] {
        let (out, err, ok) = axon(&["infra", &source_for(target), "--target", target]);
        assert!(ok, "{target}: {err}");
        let mut checked = 0;
        let chars: Vec<char> = page.chars().collect();
        let pre: Vec<char> = prefix.chars().collect();
        for i in 0..chars.len() {
            if !chars[i..].starts_with(&pre[..]) {
                continue;
            }
            // a resource name runs to the first character that cannot be in one
            let end = chars[i..]
                .iter()
                .position(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_'))
                .map_or(chars.len(), |p| i + p);
            let name: String = chars[i..end].iter().collect();
            if name.len() < prefix.len() + 4 || !name.contains('_') {
                continue;
            }
            assert!(
                out.contains(&name),
                "the page says `{target}` emits `{name}` and it does not. Either \
                 the renderer stopped emitting it or the page describes another tool"
            );
            checked += 1;
        }
        assert!(
            checked >= 6,
            "only {checked} `{prefix}*` resources checked for {target}"
        );
    }

    // and the k8s objects, which are kinds and not resources
    let (k8s, err, ok) = axon(&["infra", &source_for("k8s"), "--target", "k8s"]);
    assert!(ok, "{err}");
    for kind in [
        "Broker",
        "Trigger",
        "HTTPRoute",
        "Gateway",
        "Deployment",
        "NetworkPolicy",
        "CronJob",
        "ExternalSecret",
    ] {
        assert!(
            page.contains(kind),
            "the page does not name the `{kind}` that k8s emits"
        );
        assert!(
            k8s.contains(&format!("kind: {kind}")),
            "the page names `{kind}` and the k8s target does not emit it"
        );
    }
}

/// The book and the README also quote the DEMO's output, which is the strongest
/// thing either of them says: those lines are measured against containers, not
/// claimed. So they get the same treatment as the tool's messages — four
/// consecutive words of each quoted line have to appear in a script that prints
/// it.
///
/// It is the same failure mode from the other side: the pages showed a demo that
/// had stopped existing —two example services, a pgdog that was not wired yet—
/// and nothing said so, because prose is not executable.
#[test]
fn the_docs_quote_demo_output_a_script_really_prints() {
    // interpolations get REMOVED, not turned into words: the script says
    // `${elapsed}ms inside the ${budget}ms budget` and the book quotes
    // `14000ms inside the 60000ms budget`. Deleting them leaves the same words
    // on both sides, which is the part worth comparing.
    let plain = |t: &str, code: bool| -> String {
        let mut src = t.to_string();
        if code {
            // ${...}, $name and {name}
            while let Some(i) = src.find("${") {
                match src[i..].find('}') {
                    Some(j) => src.replace_range(i..i + j + 1, " "),
                    None => break,
                }
            }
            while let Some(i) = src.find("{") {
                match src[i..].find('}') {
                    Some(j)
                        if src[i + 1..i + j]
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c == '_') =>
                    {
                        src.replace_range(i..i + j + 1, " ")
                    }
                    _ => {
                        src.replace_range(i..i + 1, " ");
                    }
                }
            }
            let mut out = String::new();
            let mut chars = src.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '$' {
                    while chars
                        .peek()
                        .is_some_and(|n| n.is_ascii_alphanumeric() || *n == '_')
                    {
                        chars.next();
                    }
                    out.push(' ');
                } else {
                    out.push(c);
                }
            }
            src = out;
        }
        let mut o = String::from(" ");
        let mut space = true;
        for c in src.chars() {
            if c.is_ascii_alphabetic() {
                o.push(c.to_ascii_lowercase());
                space = false;
            } else if !space {
                o.push(' ');
                space = true;
            }
        }
        o
    };

    // Whatever prints a line the pages quote: the demo's scripts, the example's
    // own services, and `src/` for the lines that come from the CLI itself.
    let mut corpus = String::new();
    for pattern in [
        "examples",
        "examples/services/orders",
        "examples/services/payments",
        "examples/services/checkout",
        "src",
    ] {
        for entry in std::fs::read_dir(pattern).unwrap().flatten() {
            let path = entry.path();
            let keep = path
                .extension()
                .is_some_and(|e| e == "sh" || e == "py" || e == "ts" || e == "rs");
            if keep {
                corpus.push_str(&plain(&std::fs::read_to_string(path).unwrap(), true));
            }
        }
    }

    let mut quoted = 0;
    let mut pages = Vec::new();
    for entry in std::fs::read_dir("docs/src").unwrap().flatten() {
        pages.push(entry.path());
    }
    pages.push(std::path::PathBuf::from("README.md"));
    for path in pages {
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut inside = false;
        for (n, line) in text.lines().enumerate() {
            if line.starts_with("```") {
                inside = line.starts_with("```console");
                continue;
            }
            if !inside {
                continue;
            }
            let l = line.trim();
            // the demo's own vocabulary: a check that passed, one that failed,
            // and the `i` that explains a number
            if !(l.starts_with("OK:") || l.starts_with("FAILED:") || l.starts_with("i ")) {
                continue;
            }
            quoted += 1;
            let flat = plain(l, false);
            let words: Vec<&str> = flat.split_whitespace().collect();
            let n_words = words.len().min(4);
            assert!(
                n_words >= 2,
                "{name}:{}: nothing to compare in `{l}`",
                n + 1
            );
            assert!(
                words
                    .windows(n_words)
                    .any(|w| corpus.contains(&format!(" {} ", w.join(" ")))),
                "{name}:{}: no script prints this line. Either the demo stopped \
                 printing it or it was paraphrased:\n  {l}",
                n + 1
            );
        }
    }
    assert!(
        quoted >= 40,
        "only {quoted} quoted demo lines were checked; the scan stopped seeing them"
    );
    eprintln!("{quoted} quoted demo lines checked against the scripts that print them");
}

/// The three projections nothing asserted: the event topology, the class diagram
/// and the registry. They are the answer to "who consumes this event and what
/// breaks if I change a field on it", so what is checked is that the RELATIONSHIP
/// is in them —not that they render prettily— and that an external service is
/// told apart from one of your own.
#[test]
fn the_diagrams_and_the_registry_carry_the_relationships() {
    // --- axon graph: the topology -----------------------------------------
    let (graph, err, ok) = axon(&["graph", "examples"]);
    assert!(ok, "{err}");
    // the emitter reaches its event, and the event reaches its consumer: those
    // two edges together are the chain nobody can read from five repos
    assert!(
        graph.contains("orders -- order.placed@v1 --> order_placed_v1((order.placed@v1))"),
        "the emitter's edge is missing:\n{graph}"
    );
    assert!(
        graph.contains("order_placed_v1((order.placed@v1)) --> payments"),
        "the consumer's edge is missing:\n{graph}"
    );
    // a synchronous call is a different edge from an event: reading them the
    // same way is how a distributed monolith looks like an event-driven system
    assert!(
        graph.contains("payments -. getOrder .-> orders"),
        "the synchronous dependency is missing:\n{graph}"
    );
    // and an external service is drawn as external, because you cannot change it
    assert!(graph.contains("stripe([stripe])"), "{graph}");
    assert!(
        graph.contains("payments -. charges.create .-> stripe"),
        "{graph}"
    );

    // --- axon classes: the same manifest, another projection ---------------
    let (classes, err, ok) = axon(&["classes", "examples"]);
    assert!(ok, "{err}");
    assert!(classes.starts_with("classDiagram"), "{classes}");
    // services and events are different kinds of thing, and the diagram says so
    assert!(
        classes.contains("class PaymentsService {") && classes.contains("<<service>>"),
        "{classes}"
    );
    assert!(
        classes.contains("class OrderPlacedV1 {") && classes.contains("<<event>>"),
        "{classes}"
    );
    // emitting is a dependency and calling is another: `..>` vs `-->`
    assert!(
        classes.contains("OrdersService ..> OrderPlacedV1 : emits"),
        "the emission is missing:\n{classes}"
    );
    assert!(
        classes.contains("PaymentsService --> OrdersService : getOrder"),
        "the call is missing:\n{classes}"
    );
    // the method's signature comes from the contract, not from the code
    assert!(
        classes.contains("+placeOrder(PlaceOrderIn) PlaceOrderOut"),
        "{classes}"
    );

    // --- axon discover: the registry --------------------------------------
    let (json, err, ok) = axon(&["discover", "examples"]);
    assert!(ok, "{err}");
    let v: serde_json::Value = serde_json::from_str(&json).expect("registry json");
    for svc in ["orders", "payments", "checkout", "stripe"] {
        assert!(!v[svc].is_null(), "{svc} is not in the registry:\n{json}");
    }
    // what it is for: which method, what goes in, what comes out
    assert_eq!(v["orders"]["methods"]["placeOrder"]["in"]["total"], "money");
    assert_eq!(
        v["orders"]["methods"]["placeOrder"]["out"]["orderId"],
        "uuid"
    );
    // and who emits and who consumes, which is the question that starts it all
    assert!(
        v["orders"]["emits"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("order.placed@v1")),
        "{json}"
    );
    assert!(
        v["payments"]["consumes"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("order.placed@v1")),
        "{json}"
    );
    // an external service is marked as such: a contract you can read and cannot
    // change is not the same as one of your own
    assert_eq!(v["stripe"]["external"], true);
    assert_eq!(v["orders"]["external"], false);
    // and every entry says where it came from, so a registry merged from disk
    // and from live services can be told apart
    assert!(
        v["orders"]["source"]
            .as_str()
            .unwrap()
            .ends_with("orders.toml"),
        "{json}"
    );
}

/// A declared metric is a view next to the funnels, in the dialect of each
/// warehouse, and it has to be valid SQL in all three: a `sum` over a `money`
/// field adds up its amount column, and the time bucket is a different function
/// in every one of them.
#[test]
fn the_declared_metrics_are_valid_sql() {
    for (target, bucket, quote) in [
        ("bigquery", "TIMESTAMP_TRUNC(event_time, DAY)", '`'),
        ("snowflake", "DATE_TRUNC('DAY', event_time)", '"'),
        ("clickhouse", "toStartOfDay(event_time)", '"'),
    ] {
        let (ddl, err, ok) = axon(&["analytics", "examples", "--target", target]);
        assert!(ok, "{target}: {err}");
        assert!(
            ddl.contains(&format!("VIEW {quote}@dataset.metric_gmv{quote}")),
            "{target}: no metric view:\n{ddl}"
        );
        // the bucket is per dialect: the three truncate a timestamp differently
        assert!(
            ddl.contains(bucket),
            "{target}: no bucket `{bucket}`:\n{ddl}"
        );
        // `total` is `money`, so what gets added up is its amount column
        assert!(
            ddl.contains("sum(total_amount) AS value"),
            "{target}: the money field is not added up by its amount:\n{ddl}"
        );
        // and a `money` sub-field is a dimension, flat in the warehouse
        assert!(
            ddl.contains("GROUP BY bucket, total_currency"),
            "{target}: the dimension did not arrive:\n{ddl}"
        );
        // a count counts rows, and reads the table directly
        assert!(ddl.contains("count(*) AS value"), "{target}:\n{ddl}");
        assert!(
            !ddl.contains("FROM SELECT"),
            "{target}: a single event should read its table, not a subquery:\n{ddl}"
        );

        // And that it parses with THAT warehouse's dialect. A view that does not
        // parse fails at apply time, which is after somebody trusted the number.
        let sql = ddl.replace("@dataset", "ds");
        let parsed = match target {
            "bigquery" => {
                sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::BigQueryDialect {}, &sql)
            }
            "snowflake" => {
                sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::SnowflakeDialect {}, &sql)
            }
            _ => sqlparser::parser::Parser::parse_sql(
                &sqlparser::dialect::ClickHouseDialect {},
                &sql,
            ),
        };
        assert!(
            parsed.is_ok(),
            "{target}: the metric's SQL does not parse: {}",
            parsed.unwrap_err()
        );
    }

    // the neutral plan carries the metrics too, for whoever renders it themselves
    let (plan, _, _) = axon(&["analytics", "examples", "--target", "plan"]);
    assert!(
        plan.contains("\"gmv\"") && plan.contains("\"orders_placed\""),
        "the plan does not carry the declared metrics:\n{plan}"
    );
}

/// The metric rules: each one blocks a way of having a number that answers
/// something other than what it claims. None of them fails at apply time —a sum
/// over text answers zero in one warehouse— so the compiler is the only place
/// they can be caught.
#[test]
fn the_metric_rules_block() {
    let dir = std::env::temp_dir().join("axon-metrics");
    let base = r#"service = "shop"
version = "1.0.0"
owner = "team"
tier = "1"

[emits."order.placed@v1"]
orderId = "uuid"
customerEmail = "string"
total = "money"

[analytics]
warehouse = "clickhouse"
"#;
    let run = |extra: &str| -> String {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shop.toml"), format!("{base}{extra}")).unwrap();
        let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        assert!(!ok, "it passed clean:\n{extra}");
        err
    };

    // a metric over an event nobody emits counts zero forever
    let err = run("[metrics.m]
on = [\"other.thing@v1\"]
kind = \"count\"
");
    assert!(err.contains("nobody emits it"), "{err}");
    assert!(err.contains("counts zero forever"), "{err}");

    // a sum over something that is not a number
    let err = run("[metrics.m]
on = [\"order.placed@v1\"]
kind = \"sum\"
field = \"orderId\"
");
    assert!(err.contains("is `uuid` in `order.placed@v1`"), "{err}");
    assert!(err.contains("answers zero in another"), "{err}");

    // a sum with no field has nothing to add up
    let err = run("[metrics.m]
on = [\"order.placed@v1\"]
kind = \"sum\"
");
    assert!(err.contains("with no `field`"), "{err}");

    // a dimension the event does not declare becomes a NULL group
    let err = run("[metrics.m]
on = [\"order.placed@v1\"]
kind = \"count\"
by = [\"region\"]
");
    assert!(err.contains("does not declare"), "{err}");
    assert!(err.contains("NULL group"), "{err}");

    // and the one that matters: grouping by a personal field is a lookup table.
    // `pii` is a field of the service, so it goes BEFORE any table: appended at
    // the end it would land inside `[analytics]`, where `pii` is a mode.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("shop.toml"),
        format!(
            "pii = [\"customer_email\"]\n{base}[metrics.m]\non = [\"order.placed@v1\"]\nkind = \"count\"\nby = [\"customerEmail\"]\n"
        ),
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok, "grouping by a personal field passed clean:\n{err}");
    assert!(err.contains("declares as `pii`"), "{err}");
    assert!(err.contains("hashing it does not change that"), "{err}");

    // an aggregation or a bucket that does not exist in the three warehouses
    let err = run("[metrics.m]
on = [\"order.placed@v1\"]
kind = \"p95\"
field = \"total\"
");
    assert!(err.contains("is not an aggregation"), "{err}");
    let err = run("[metrics.m]
on = [\"order.placed@v1\"]
kind = \"count\"
window = \"13min\"
");
    assert!(err.contains("is not a bucket"), "{err}");

    // a metric while the service exports nothing reads tables that carry nothing.
    // `[analytics]` is already in the base, so this variant edits it instead of
    // declaring it twice.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("shop.toml"),
        format!(
            "{}\n[metrics.m]\non = [\"order.placed@v1\"]\nkind = \"count\"\n",
            base.replace(
                "warehouse = \"clickhouse\"",
                "warehouse = \"clickhouse\"\nexport = false"
            )
        ),
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok, "a metric with no export passed clean:\n{err}");
    assert!(err.contains("carry nothing"), "{err}");

    // and a count with a field is a warning: the field is ignored, and nobody
    // reading the declaration would guess so
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("shop.toml"),
        format!(
            "{base}[metrics.m]
on = [\"order.placed@v1\"]
kind = \"count\"
field = \"total\"
"
        ),
    )
    .unwrap();
    let (out, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "a warning should not block:\n{err}");
    assert!(
        format!("{out}{err}").contains("A count counts rows"),
        "{out}{err}"
    );
}

/// The data warehouse: one table per event and the funnel views, which come
/// out of the DECLARED causal chain. That last part is the one no warehouse
/// has: a funnel is normally assembled by guessing how the events relate.
#[test]
fn the_warehouse_schemas_are_valid_sql() {
    let (ddl, err, ok) = axon(&["analytics", "examples", "--target", "bigquery"]);
    assert!(ok, "{err}");

    // the envelope: without correlation_id there is no funnel
    for col in [
        "event_id",
        "event_time TIMESTAMP",
        "correlation_id",
        "causation_id",
    ] {
        assert!(ddl.contains(col), "the {col} column is missing:\n{ddl}");
    }
    // partitioning is not optional: without it every query scans the history
    assert!(ddl.contains("PARTITION BY DATE(event_time)"), "{ddl}");
    assert!(ddl.contains("CLUSTER BY correlation_id, source"), "{ddl}");
    // warehouse convention: snake_case, not the contract's camelCase
    assert!(
        ddl.contains("customer_id STRING") && !ddl.contains("customerId STRING"),
        "{ddl}"
    );
    // `money` is flattened, because you cannot sum an object; and
    // `amount_amount` adds nothing
    assert!(
        ddl.contains("total_amount INT64") && ddl.contains("total_currency STRING"),
        "{ddl}"
    );
    assert!(
        ddl.contains("\n  amount INT64"),
        "amount_amount was not collapsed:\n{ddl}"
    );
    // the example's hash mode: the hash is exported, not the value
    assert!(ddl.contains("customer_email_hash STRING"), "{ddl}");
    assert!(
        !ddl.contains("customer_email STRING"),
        "the address was exported in plaintext:\n{ddl}"
    );

    // the funnel comes from the declared chain, with the business latency
    assert!(
        ddl.contains("CREATE OR REPLACE VIEW `@dataset.funnel_order_placed_v1`"),
        "{ddl}"
    );
    assert!(ddl.contains("AS step_1_order_placed_v1"), "{ddl}");
    assert!(ddl.contains("AS step_2_payment_captured_v1"), "{ddl}");
    assert!(
        ddl.contains("AS ms_to_payment_captured_v1"),
        "no business latency:\n{ddl}"
    );

    // And what matters: that it is valid SQL. A comment at the end of a column
    // swallows the comma separating it from the next one, and the DDL ends up
    // broken —it had already happened to me in a migration, and it happened again here.
    let sql = ddl.replace("@dataset", "ds");
    let sentencias =
        sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::BigQueryDialect {}, &sql);
    assert!(
        sentencias.is_ok(),
        "the warehouse DDL is not valid SQL: {}",
        sentencias.unwrap_err()
    );
    assert!(sentencias.unwrap().len() >= 3, "faltan sentencias");

    // the neutral plan, for whoever does not use BigQuery
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

    // and the native sink: Pub/Sub writes straight in, with no intermediate process
    let (g, _, _) = axon(&["infra", &source_for("gcp"), "--target", "gcp"]);
    assert!(g.contains("bigquery_config"), "{g}");
    assert!(g.contains("use_table_schema = true"), "{g}");
    // the warehouse needs a DLQ too: a message that does not fit does not vanish
    let warehouse = g
        .split("_warehouse\" {")
        .nth(1)
        .expect("the warehouse subscription");
    assert!(
        warehouse.contains("dead_letter_policy"),
        "the warehouse sink with no DLQ:\n{warehouse}"
    );
}

/// Sharding rules no other tool enforces: PgDog's schema validator is on its
/// roadmap unstarted and Citus only fails at runtime when distributing. Each
/// one describes a collision or a leak that raises NO error, only wrong data.
#[test]
fn the_sharding_rules_block() {
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

    // a UNIQUE that does not include the key: each node honours it, the set does not
    let (msg, ok) = probar(
        "CREATE TABLE cuenta (id uuid PRIMARY KEY, tenant_id uuid NOT NULL,\n  \
         handle varchar(64) NOT NULL UNIQUE);\n",
        base,
    );
    assert!(!ok);
    assert!(
        msg.contains("`UNIQUE (handle)` does not include `tenant_id`"),
        "{msg}"
    );

    // a composite one that DOES include it is safe, and a uuid PK is unique by
    // construction: without those two exceptions the rule would be noise, and
    // a rule with false positives gets silenced
    let (msg, ok) = probar(
        "CREATE TABLE cuenta (id uuid PRIMARY KEY, tenant_id uuid NOT NULL,\n  \
         handle varchar(64) NOT NULL,\n  CONSTRAINT u UNIQUE (tenant_id, handle));\n",
        base,
    );
    assert!(
        ok,
        "a composite UNIQUE including the key should not fail:\n{msg}"
    );

    // one sequence per node starts at 1 on each of them
    let (msg, ok) = probar(
        "CREATE TABLE cuenta (id uuid PRIMARY KEY, tenant_id uuid NOT NULL, numero bigserial);\n",
        base,
    );
    assert!(!ok);
    assert!(
        msg.contains("generated from a sequence (`bigserial`)"),
        "{msg}"
    );

    // isolating by one column and sharding by another makes every query of one
    // tenant touch every node
    let (msg, ok) = probar(
        "CREATE TABLE cuenta (id uuid PRIMARY KEY, tenant_id uuid, cliente_id uuid);\n",
        &base.replace(
            "tenant_column = \"tenant_id\"",
            "tenant_column = \"cliente_id\"",
        ),
    );
    assert!(!ok);
    assert!(
        msg.contains("isolates by `cliente_id` and shards by `tenant_id`"),
        "{msg}"
    );

    // N nodes are N timelines: there is no global recovery point
    let (msg, ok) = probar(
        "CREATE TABLE cuenta (id uuid PRIMARY KEY, tenant_id uuid NOT NULL);\n",
        &format!("{base}pitr = true\nbackup_retention_days = 7\n"),
    );
    assert!(!ok);
    assert!(msg.contains("no consistent recovery point"), "{msg}");
}

/// The engine has to exist. `state = "neo4j"` used to pass `verify` with no
/// error and generate a Cloud SQL Postgres instance: wrong output, silently,
/// which is the worst failure mode there is.
/// `verify` does the connection arithmetic against `max_connections`, so that
/// number has to be APPLIED. A rule comparing against a ceiling nobody sets is
/// comparing against the engine's default, which is lower.
#[test]
fn the_connection_ceiling_is_applied() {
    let (g, _, _) = axon(&["infra", &source_for("gcp"), "--target", "gcp"]);
    assert!(
        g.contains("name  = \"max_connections\""),
        "gcp does not apply the ceiling:\n{g}"
    );
    assert!(
        g.contains("value = \"200\""),
        "payments' value did not arrive:\n{g}"
    );
    let (a, _, _) = axon(&["infra", &source_for("aws"), "--target", "aws"]);
    // on RDS the ceiling goes in a parameter group, not on the instance
    assert!(a.contains("resource \"aws_db_parameter_group\""), "{a}");
    assert!(
        a.contains("parameter_group_name    = aws_db_parameter_group.payments.name"),
        "{a}"
    );
    let (l, _, _) = axon(&["infra", "examples", "--target", "local"]);
    assert!(
        l.contains("\"max_connections=200\""),
        "local does not apply the ceiling: exhausting connections there is the only \
         way to find out before it scales:\n{l}"
    );
}

/// The generated `pgdog.toml` is validated against pgdog's OFFICIAL JSON
/// Schema, not against expected text. They generate it from their own Rust
/// types and their CI fails if it drifts, so validating against that file is
/// validating against the real parser that will read the configuration.
#[test]
fn the_pgdog_toml_validates_against_its_schema() {
    let (cfg, err, ok) = axon(&["pooler", "examples"]);
    assert!(ok, "{err}");

    // the sharding comes from the real schema, not from a hand-written list
    assert!(cfg.contains("[[sharded_tables]]"), "{cfg}");
    assert!(cfg.contains("column = \"tenant_id\""), "{cfg}");
    assert!(
        cfg.contains("data_type = \"uuid\""),
        "the key's type did not come from the DDL:\n{cfg}"
    );
    // the same hash as Postgres's `PARTITION BY HASH`
    assert!(cfg.contains("hasher = \"postgres\""), "{cfg}");
    // refuse rather than return an incomplete result
    assert!(cfg.contains("cross_shard_disabled = true"), "{cfg}");
    // `on` and not `auto`: in `auto` the parser does not kick in with a single
    // primary node, which is exactly where a session GUC slips through uncaught
    assert!(cfg.contains("query_parser = \"on\""), "{cfg}");
    // a generated file is no place for a host or a password
    assert!(cfg.contains("${AXON_DB_HOST_0}"), "{cfg}");
    assert!(
        !cfg.to_lowercase().contains("password ="),
        "the generated file carries a password:\n{cfg}"
    );
    // one node per shard, plus the declared read replicas
    assert_eq!(cfg.matches("[[databases]]").count(), 6, "{cfg}");
    assert!(
        cfg.contains("role = \"replica\""),
        "the declared replicas did not arrive:\n{cfg}"
    );

    if !has("python3") {
        eprintln!("skipping the rest: python3 is not installed");
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
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the generated pgdog.toml does not validate:\n{printed}"
    );
    // if the module is missing it skips, it never lies by saying it passed
    assert!(
        printed.contains("OK: valida") || printed.contains("SALTEADO"),
        "{printed}"
    );
    eprintln!("{}", printed.trim());
}

/// The pooler changes the subject of the connection arithmetic, and in
/// transaction mode it can break tenant isolation without raising an error.
#[test]
fn the_pooler_rules_block() {
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

    // THE rule: in transaction mode the connection goes back to the pool on
    // every COMMIT and is handed to another tenant
    let (msg, ok) = probar("[pooler]\nengine = \"pgdog\"\nmode = \"transaction\"\nshards = 2\n");
    assert!(!ok);
    assert!(msg.contains("no `tenant_binding = \"set_local\"`"), "{msg}");
    assert!(msg.contains("reads the previous tenant's rows"), "{msg}");

    // declaring it lets it through
    let (msg, ok) = probar(
        "[pooler]\nengine = \"pgdog\"\nmode = \"transaction\"\n\
         tenant_binding = \"set_local\"\nshards = 2\n",
    );
    assert!(ok, "declaring the binding should be enough:\n{msg}");

    // sharding without saying by which column
    let (msg, ok) = probar("[pooler]\nengine = \"pgdog\"\nmode = \"session\"\nshards = 4\n");
    let sin_clave = msg.clone();
    assert!(ok || sin_clave.contains("shards"), "{sin_clave}");

    // 2PC gives eventual consistency: promising CP on top contradicts itself
    let (msg, ok) =
        probar("[cap2]\n[pooler]\nengine = \"pgdog\"\nmode = \"session\"\nshards = 4\n");
    let _ = (msg, ok);

    // A sharder's 2PC gives eventual consistency with visible partial states:
    // promising CP on top is the same contradiction as reading from a replica
    // and promising CP. Here the manifest is rewritten whole so it can
    // declare `strong`.
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

    // in session mode one client connection ties up one server connection
    let (msg, ok) = probar(
        "pool_size = 20\nmax_connections = 500\n[pooler]\nengine = \"pgdog\"\n\
         mode = \"session\"\npool_size = 30\n",
    );
    assert!(!ok);
    assert!(msg.contains("does not multiplex"), "{msg}");

    // pooler fields with no pooler apply nowhere
    let (msg, ok) = probar("[pooler]\nshards = 4\n");
    assert!(!ok);
    assert!(msg.contains("with `engine = \"none\"`"), "{msg}");
}

#[test]
fn an_unknown_engine_does_not_generate_postgres() {
    let dir = std::env::temp_dir().join("axon-motor");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("g.toml"),
        "service = \"g\"\nowner = \"e\"\ntier = \"2\"\n[infra]\nstate = \"neo4j\"\n",
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok, "an unsupported engine has to fail");
    assert!(err.contains("no esta soportado"), "{err}");
    // and the message says how to proceed, not just that it cannot be done
    assert!(err.contains("axon-infra-neo4j"), "{err}");
    // postgres is still valid
    std::fs::write(
        dir.join("g.toml"),
        "service = \"g\"\nowner = \"e\"\ntier = \"2\"\n[infra]\nstate = \"postgres\"\n",
    )
    .unwrap();
    let (_, _, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok);
}

#[test]
fn openapi_requires_an_idempotency_key() {
    let (json, _, _) = axon(&["openapi", "examples"]);
    assert!(json.contains("Idempotency-Key"));
    assert!(
        json.contains("application/problem+json"),
        "errores no uniformes"
    );
    assert!(json.contains("/v1/payments"));
}

/// A pair of manifests with a two-step saga, one with a compensation and the
/// last one without. The coordinator test and the Terraform one both use it:
/// the saga is the only thing that makes the sweep's resources appear in the IaC.
fn fixture_saga(suffix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("axon-saga-{suffix}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sql")).unwrap();
    std::fs::write(
        dir.join("sql/001_init.expand.sql"),
        "CREATE TABLE saga_checkout (\n  id uuid PRIMARY KEY,\n  step int NOT NULL,\n  \
         status text NOT NULL,\n  data jsonb NOT NULL,\n  \
         updated timestamptz NOT NULL\n);\n",
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

# This fixture tests the saga, not the warehouse. Without this, `axon infra`
# rejects the plan because the default warehouse has no ingest path on every
# target — which is exactly what the error message suggests doing.
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

/// The sweep has to EXIST on all four targets. A coordinator that knows how
/// to resume plus a scheduler that does not get deployed is the same as not
/// having it, and the IaC applies without saying a thing.
#[test]
fn the_sweep_is_deployed_on_all_four_targets() {
    let dir = fixture_saga("targets");
    let f = dir.to_str().unwrap();
    for (target, marker) in [
        ("local", "/internal/saga/checkout/sweep"),
        ("gcp", "resource \"google_cloud_scheduler_job\""),
        ("aws", "resource \"aws_scheduler_schedule\""),
        ("k8s", "kind: CronJob"),
    ] {
        let (out, err, ok) = axon(&["infra", f, "--target", target]);
        assert!(ok, "{target}: {err}");
        assert!(out.contains(marker), "{target} does not deploy the sweep");
        // and always against the internal route, not against the edge
        assert!(
            out.contains("/internal/saga/checkout/sweep"),
            "{target} does not point at the sweep route"
        );
    }
    // The sweep route is NOT a declared route of the service, so it does not
    // go out through the gateway: it triggers compensations and cannot be public.
    let (g, _, _) = axon(&["infra", f, "--target", "gcp"]);
    assert!(
        !g.contains("google_compute_url_map"),
        "the sweep slipped into the edge"
    );
    // On k8s the network policy has to let the sweep's pod in: otherwise the
    // CronJob applies, the curl never arrives and only the history says so.
    let (k, _, _) = axon(&["infra", f, "--target", "k8s"]);
    assert!(
        k.contains("axon.dev/sweep"),
        "the sweep's pod does not get in"
    );
    assert_eq!(
        k.matches("axon.dev/sweep").count(),
        2,
        "the label goes on the pod and on the policy, not on just one"
    );
    // The snapshot prune is deployed too, and for the same reason: a
    // `snapshot_version` that invalidates snapshots with nothing to delete them
    // makes the table grow with every rules version.
    let es = fixture_es("cron");
    for (target, marker) in [
        ("local", "/internal/aggregate/cuenta/prune"),
        ("gcp", "resource \"google_cloud_scheduler_job\""),
        ("aws", "resource \"aws_scheduler_schedule\""),
        ("k8s", "kind: CronJob"),
    ] {
        let (out, err, ok) = axon(&["infra", es.to_str().unwrap(), "--target", target]);
        assert!(ok, "{target}: {err}");
        assert!(
            out.contains(marker),
            "{target} does not deploy the snapshot prune"
        );
        assert!(
            out.contains("/internal/aggregate/cuenta/prune"),
            "{target} does not point at the prune route"
        );
    }

    // The interval comes from the declared budget, and at 1 minute
    // EventBridge's unit goes in the singular: `rate(1 minutes)` does not validate.
    let (a, _, _) = axon(&["infra", f, "--target", "aws"]);
    assert!(a.contains("rate(1 minute)"), "{a}");
}

/// A saga is not validated by reading the generated code: it is run. What has
/// to happen when step 2 fails is that step 1 ends up UNDONE, and that the
/// journal says the saga compensated. That cannot be asserted over text.
#[test]
fn the_generated_saga_compensates_in_reverse() {
    if !has("node") {
        eprintln!("salteado: falta node");
        return;
    }
    let dir = fixture_saga("compensa");
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "the saga fixture is not clean:\n{err}");

    // the coordinator is generated and run with Node's testkit, in the
    // example's own directory so its node_modules can be reused
    let (ts, err, ok) = axon(&[
        "build",
        dir.join("almacen.toml").to_str().unwrap(),
        dir.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    // its own directory: the generated testkit imports no dependency —only
    // `node:test` and the generated module— and writing into the example's
    // makes this test and the typecheck collide when they run in parallel
    let target_dir = dir.join("prueba");
    std::fs::create_dir_all(&target_dir).unwrap();
    let target_dir = target_dir.as_path();
    std::fs::write(target_dir.join("axon.saga.contratos.ts"), &ts).unwrap();
    std::fs::write(
        target_dir.join("axon.saga.test.ts"),
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

/** Fake actions: they record the order and fail when told to. */
function acciones(rompe: string[]) {
  const done: string[] = [];
  const a: CheckoutActions = {
    async step1Cobrar() {
      if (rompe.includes("cobrar")) throw new Error("cobrar");
      done.push("cobrar");
      return { ok: "cobrado" };
    },
    async undo1Reembolsar(_e: Envelope<unknown>, prior: CheckoutOutputs) {
      if (rompe.includes("reembolsar")) throw new Error("reembolsar");
      // the suffix is what proves the compensation received step 1's output
      // —and, after a resume, that it came from the journal and not a variable
      done.push(`reembolsar:${prior.step1?.ok ?? "sin-cobro"}`);
    },
    async step2PagarProveedor() {
      if (rompe.includes("pagar")) throw new Error("pagar");
      done.push("pagar");
      return { ok: "pagado" };
    },
  };
  return { a, done };
}

test("the happy path compensates nothing", async () => {
  const { a, done } = acciones([]);
  const d = new Journal();
  const r = await runCheckout("s1", a, d, newEnvelope("x@v1", "prueba", {}));
  assert.equal(r.status, "completed");
  assert.deepEqual(done, ["cobrar", "pagar"]);
  assert.equal(d.final, "completed");
});

test("if step 2 fails, step 1 is undone", async () => {
  const { a, done } = acciones(["pagar"]);
  const d = new Journal();
  const r = await runCheckout("s2", a, d, newEnvelope("x@v1", "prueba", {}));
  assert.equal(r.status, "compensated");
  // order matters: charging happened first, and the last thing that ran was its inverse
  // the suffix proves the compensation RECEIVED what step 1 returned
  assert.deepEqual(done, ["cobrar", "reembolsar:cobrado"]);
  assert.equal(d.final, "compensated");
});

test("if step 1 fails, there is nothing done to undo", async () => {
  const { a, done } = acciones(["cobrar"]);
  const d = new Journal();
  const r = await runCheckout("s3", a, d, newEnvelope("x@v1", "prueba", {}));
  assert.equal(r.status, "compensated");
  // it was attempted, so it gets undone all the same: the compensation tolerates finding nothing
  assert.deepEqual(done, ["reembolsar:sin-cobro"]);
});

test("a compensation that fails leaves the saga stuck, and it shows", async () => {
  const { a } = acciones(["pagar", "reembolsar"]);
  const d = new Journal();
  await assert.rejects(
    () => runCheckout("s4", a, d, newEnvelope("x@v1", "prueba", {})),
    (err: unknown) => err instanceof SagaStuck && err.step === 1,
  );
  assert.equal(d.final, "stuck");
});

test("the sweep resumes a stranded saga and compensates it", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", { orderId: "o-1" });
  // a saga left with step 1 in `attempting`: the process that had it in flight
  // died right after calling and before recording the result
  d.rows.set("colgada", { step: 1, status: "attempting", data: e, updatedAt: 0 });
  const { a, done } = acciones([]);
  const r = await sweepCheckout(a, d);
  assert.equal(r.claimed, 1);
  assert.equal(r.compensated, 1);
  assert.equal(r.completed, 0);
  assert.equal(r.pending, false);
  // a step in doubt is not retried: it is undone
  assert.deepEqual(done, ["reembolsar:sin-cobro"]);
});

test("on resume, the compensation receives what the journal saved", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", {});
  // step 1 was left DONE in another process, and its output is in the journal
  d.rows.set("colgada", {
    step: 1, status: "done", data: e, updatedAt: 0,
    outputs: { 1: { ok: "cobrado" } },
  });
  // step 2 fails, so step 1 has to be undone with what step 1 returned
  const { a, done } = acciones(["pagar"]);
  const r = await sweepCheckout(a, d);
  assert.equal(r.compensated, 1);
  // "sin-cobro" here would mean the journal's cast left everything undefined
  assert.deepEqual(done, ["reembolsar:cobrado"]);
});

test("the sweep does not touch a saga that is on its way", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", {});
  d.rows.set("viva", { step: 1, status: "attempting", data: e, updatedAt: Date.now() });
  const { a, done } = acciones([]);
  const r = await sweepCheckout(a, d);
  // sweeping a live saga would be a second coordinator over the same steps
  assert.equal(r.claimed, 0);
  assert.deepEqual(done, []);
});

test("claim claims: the second sweeper does not see the same saga", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", {});
  d.rows.set("colgada", { step: 1, status: "done", data: e, updatedAt: 0 });
  const before = new Date(Date.now() - 20000);
  const primero = await d.claim("checkout", before, 50);
  const segundo = await d.claim("checkout", before, 50);
  assert.equal(primero.length, 1);
  assert.equal(segundo.length, 0);
});

test("a stuck saga is counted and not retried", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", {});
  d.rows.set("colgada", { step: 1, status: "attempting", data: e, updatedAt: 0 });
  const { a, done } = acciones(["reembolsar"]);
  const r = await sweepCheckout(a, d);
  // the pass does not abort: it counts the stuck one and carries on
  assert.equal(r.stuck, 1);
  assert.equal(d.final, "stuck");
  assert.deepEqual(done, []);
  // and once closed as stuck, the next sweep does not take it again
  const other = await sweepCheckout(a, d);
  assert.equal(other.claimed, 0);
});

test("if the limit fills up, the sweep says so", async () => {
  const d = new Journal();
  const e = newEnvelope("x@v1", "prueba", {});
  for (const id of ["a", "b", "c"]) {
    d.rows.set(id, { step: 1, status: "attempting", data: e, updatedAt: 0 });
  }
  const { a } = acciones([]);
  const r = await sweepCheckout(a, d, 2);
  assert.equal(r.claimed, 2);
  // a silent cap reads as "there was nothing more"
  assert.equal(r.pending, true);
});

test("the last step carries no compensation, and the rest do", () => {
  assert.equal(checkoutSteps.length, 2);
  assert.equal(checkoutSteps[0].undo, "banco.reembolsar");
  assert.equal(checkoutSteps[1].undo, null);
});
"#,
    )
    .unwrap();
    let out = Command::new("node")
        .args(["--test", "axon.saga.test.ts"])
        .current_dir(target_dir)
        .output()
        .expect("node --test");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the generated coordinator does not compensate as it claims:\n{printed}"
    );
    assert!(printed.contains("pass 11"), "{printed}");
}

/// The saga rules: each one blocks a different way of ending up half done.
/// Without them the saga still gets generated and fails the day something has
/// to be compensated, which is the worst day to find out.
#[test]
fn the_saga_rules_block() {
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
    let run = |saga: &str, ddl: &str| -> String {
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
                         step int NOT NULL,\n  state text NOT NULL,\n  \
                         data jsonb NOT NULL,\n  updated timestamptz NOT NULL\n);\n";

    // an intermediate step with no compensation
    let err = run(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar" },
  { do = "banco.pagarProveedor" },
]"#,
        TABLA,
    );
    assert!(
        err.contains("has no `undo`, and it is not the last one"),
        "{err}"
    );
    assert!(err.contains("dual-write with more steps"), "{err}");

    // a compensation that is not idempotent
    let err = run(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.pagarProveedor" },
  { do = "banco.reembolsar" },
]"#,
        TABLA,
    );
    assert!(err.contains("is not `idempotent`"), "{err}");
    assert!(err.contains("applies the effect twice"), "{err}");

    // the budget does not cover the sum of the steps
    let err = run(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 1000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]"#,
        TABLA,
    );
    assert!(err.contains("add up to 11000ms"), "{err}");
    assert!(
        err.contains("compensating something that later succeeds"),
        "{err}"
    );

    // without the journal's table, a restart loses the saga
    let err = run(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]"#,
        "CREATE TABLE otra (id uuid PRIMARY KEY);\n",
    );
    assert!(
        err.contains("the `saga_checkout` table is missing"),
        "{err}"
    );

    // without `data` it cannot be resumed: the actions need the call, and the
    // process that had it in memory is the one that died
    let err = run(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]"#,
        "CREATE TABLE saga_checkout (\n  id uuid PRIMARY KEY,\n  step int NOT NULL,\n  \
         status text NOT NULL,\n  updated timestamptz NOT NULL\n);\n",
    );
    assert!(err.contains("has no `data` column"), "{err}");
    assert!(err.contains("goes"), "{err}");

    // and a date stored as text: the sweep's comparison compiles and sorts
    // wrong, so it would skip stranded sagas without saying anything
    let err = run(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.reembolsar" },
  { do = "banco.pagarProveedor" },
]"#,
        "CREATE TABLE saga_checkout (\n  id uuid PRIMARY KEY,\n  step int NOT NULL,\n  \
         status text NOT NULL,\n  data jsonb NOT NULL,\n  \
         updated text NOT NULL\n);\n",
    );
    assert!(err.contains("has to be timestamp"), "{err}");
    assert!(err.contains("would skip stranded sagas"), "{err}");

    // an `undo` that does not exist
    let err = run(
        r#"[saga.checkout]
on = "checkout"
timeout_ms = 20000
steps = [
  { do = "banco.cobrar", undo = "banco.devolver" },
  { do = "banco.pagarProveedor" },
]"#,
        TABLA,
    );
    assert!(err.contains("`banco` does not offer `devolver`"), "{err}");

    // a step nobody declared as a dependency: there is nothing to call it with
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
    assert!(
        err.contains("without declaring it in `[[depends]]`"),
        "{err}"
    );
    assert!(err.contains("The resilient client"), "{err}");

    // and a saga under `consistency = "strong"`
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
    assert!(
        err.contains("the real guarantee of the flow is eventual"),
        "{err}"
    );
}

/// A manifest with event sourcing and a view. The `fold` test and the rules
/// test both use it.
fn fixture_es(suffix: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("axon-es-{suffix}"));
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
  -- Without this UNIQUE, two concurrent writes on the same stream both get in
  -- with the same version and nobody sees an error.
  UNIQUE (stream_id, version)
);

CREATE TABLE cuenta_snapshot (
  stream_id  uuid NOT NULL,
  version    int  NOT NULL,
  rules     int  NOT NULL,
  state     jsonb NOT NULL,
  PRIMARY KEY (stream_id, version, rules)
);

CREATE TABLE view_saldos (
  stream_id  uuid PRIMARY KEY,
  centavos   bigint NOT NULL,
  posicion   bigint NOT NULL
);

-- The shadow: the same shape as the view. `verify` checks they MATCH, because a
-- shadow with one column fewer leaves an incomplete view after the swap, and
-- that would show up on the day of the rebuild.
CREATE TABLE view_saldos_shadow (
  stream_id  uuid PRIMARY KEY,
  centavos   bigint NOT NULL,
  posicion   bigint NOT NULL
);

CREATE TABLE view_saldos_checkpoint (
  view_name  text NOT NULL,
  -- per STREAM: an event's version is its position inside ITS stream, so a
  -- single number for the whole view identifies nothing as soon as there is
  -- more than one stream
  stream_id  uuid NOT NULL,
  position   bigint NOT NULL,
  PRIMARY KEY (view_name, stream_id)
);
";

const MANIFIESTO_ES: &str = r#"service = "libro"
version = "1.0.0"
owner = "equipo"
tier = "1"

# This fixture tests the stream, the view and the snapshots, not the warehouse:
# without this `axon infra` rejects the plan because the default warehouse has
# no ingest path on every target.
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

# The aggregate's events get published, and the stream is already durable: the
# handover to the bus goes in the same transaction as the append. `verify`
# requires it.
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

/// The `fold` is not validated by reading the switch: it is run. What has to
/// happen is that a gap in the versions BLOWS UP instead of giving a state
/// that never existed, and that an undeclared event is not ignored.
#[test]
fn the_generated_fold_rebuilds_and_refuses() {
    if !has("node") {
        eprintln!("salteado: falta node");
        return;
    }
    let dir = fixture_es("fold");
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "the fixture is not clean:\n{err}");

    let (ts, err, ok) = axon(&[
        "build",
        dir.join("libro.toml").to_str().unwrap(),
        dir.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    let target_dir = dir.join("prueba");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(target_dir.join("contratos.ts"), &ts).unwrap();
    std::fs::write(
        target_dir.join("es.test.ts"),
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

test("the state comes from the stream, not from a row", () => {
  const r = cuentaFold(rules, "c1", [
    ev(1, "cuenta.abierta@v1", { streamId: "c1" }),
    ev(2, "cuenta.depositada@v1", { streamId: "c1", centavos: 500 }),
    ev(3, "cuenta.depositada@v1", { streamId: "c1", centavos: 250 }),
  ]);
  assert.equal(r.version, 3);
  assert.deepEqual(r.state, { abierta: true, centavos: 750, cerrada: false });
});

test("a gap in the versions blows up instead of giving a state that never existed", () => {
  assert.throws(
    () => cuentaFold(rules, "c1", [
      ev(1, "cuenta.abierta@v1", { streamId: "c1" }),
      ev(3, "cuenta.depositada@v1", { streamId: "c1", centavos: 500 }),
    ]),
    /expected version 2 and got 3/,
  );
});

test("an event the manifest does not declare is not ignored", () => {
  assert.throws(
    () => cuentaFold(rules, "c1", [ev(1, "cuenta.robada@v1", {})]),
    /is not a declared event of the aggregate/,
  );
});

test("from a snapshot, the fold carries on from there", () => {
  const r = cuentaFold(rules, "c1",
    [ev(8, "cuenta.depositada@v1", { streamId: "c1", centavos: 100 })],
    { version: 7, state: { abierta: true, centavos: 900, cerrada: false } });
  assert.equal(r.version, 8);
  assert.equal(r.state.centavos, 1000);
});

test("the aggregate's events are the manifest's", () => {
  assert.deepEqual([...cuentaEvents],
    ["cuenta.abierta@v1", "cuenta.depositada@v1", "cuenta.cerrada@v1"]);
});

test("a snapshot of another rules version is not used", async () => {
  const events = [
    ev(1, "cuenta.abierta@v1", { streamId: "c1" }),
    ev(2, "cuenta.depositada@v1", { streamId: "c1", centavos: 500 }),
    ev(3, "cuenta.depositada@v1", { streamId: "c1", centavos: 250 }),
  ];
  // A stream with ONE saved snapshot, of the wrong rules version. What has to
  // happen is that it does not get used: rehydrating from there would give 99999 cents.
  const stream = {
    pedidas: [] as number[],
    async read(_id: string, from = 0) { return events.filter((e) => e.version > from); },
    async append() { return 0; },
    async snapshot(_id: string, rules: number) {
      this.pedidas.push(rules);
      // it only returns the one for the requested version, like the generated SQL
      return rules === 1 ? { version: 2, state: { abierta: true, centavos: 99999, cerrada: false } } : null;
    },
    async saveSnapshot() {},
  };
  const r = await cuentaLoad(rules, stream, "c1");
  // it asked for the current version, not the one that was saved
  assert.deepEqual(stream.pedidas, [cuentaSnapshotRules]);
  assert.notEqual(cuentaSnapshotRules, 1);
  // and the state came from the whole stream, not from the poisoned snapshot
  assert.equal(r.state.centavos, 750);
  assert.equal(r.version, 3);
});

test("snapshots are taken only at the declared cadence", async () => {
  const saved: number[] = [];
  const stream = {
    async read() { return []; },
    async append() { return 0; },
    async snapshot() { return null; },
    async saveSnapshot(_id: string, version: number) { saved.push(version); },
  };
  const state = { abierta: true, centavos: 1, cerrada: false };
  for (let v = 0; v <= 4; v++) {
    await cuentaSnapshot(stream, "c1", v, state);
  }
  // every 2, and never at version 0: a snapshot of the initial state caches nothing
  assert.deepEqual(saved, [2, 4]);
});

test("rebuilding prepares the shadow, restores the dates and swaps at the end", async () => {
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
        // this one is NOT the view's: the view only declares opened and deposited
        { version: 3, type: "cuenta.cerrada@v1", data: { streamId: "c1" }, at: "2020-01-03T00:00:00.000Z" },
      ];
    },
    async append() { return 0; },
  };
  const aplicados = await rebuildSaldos(projection, stream);
  // prepare goes FIRST and the swap AT THE END: until that moment nobody sees
  // anything of the rebuild, which is the whole point of the shadow
  assert.equal(order[0], "prepare");
  assert.equal(order[order.length - 1], "swap");
  // and the date is the STREAM's, not now's: filling it in would rewrite
  // history in silence
  assert.deepEqual(order.slice(1, -1), [
    "abierta:2020-01-01T00:00:00.000Z:1",
    "deposito:500:2",
  ]);
  // the event the view does not declare is skipped, it does not blow up: it is
  // in the stream by its own right
  assert.equal(aplicados, 2);
});

test("the prune asks for the current version, not any old one", async () => {
  let asked = -1;
  const stream = {
    async read() { return []; },
    async append() { return 0; },
    async snapshot() { return null; },
    async saveSnapshot() {},
    async pruneSnapshots(rules: number) { asked = rules; return 7; },
  };
  const borradas = await pruneCuenta(stream);
  // Asking for another version would delete exactly the ones that ARE used,
  // and the symptom would be everything rebuilding from the stream with
  // nobody knowing why.
  assert.equal(asked, cuentaSnapshotRules);
  assert.equal(borradas, 7);
  assert.equal(pruneRouteCuenta, "POST /internal/aggregate/cuenta/prune");
});

test("the view only accepts the events it declares, and the position reaches it", async () => {
  const vistas: string[] = [];
  const projection: SaldosProjection = {
    async applyCuentaAbiertaV1(e, posicion) { vistas.push(`abierta:${posicion}`); },
    async applyCuentaDepositadaV1(e, posicion) { vistas.push(`deposito:${e.data.centavos}:${posicion}`); },
  };
  await saldosApply(projection, newEnvelope("cuenta.abierta@v1", "p", { streamId: "c1" }), 11);
  await saldosApply(projection, newEnvelope("cuenta.depositada@v1", "p", { streamId: "c1", centavos: 300 }), 12);
  assert.deepEqual(vistas, ["abierta:11", "deposito:300:12"]);
  // `cuenta.cerrada@v1` is NOT in the view: arriving here would be a
  // subscription nobody asked for
  await assert.rejects(
    () => saldosApply(projection, newEnvelope("cuenta.cerrada@v1", "p", {}), 13),
    /is not a declared event of the view/,
  );
  assert.equal(saldosTable, "view_saldos");
  assert.equal(saldosMaxStalenessMs, 3000);
});
"#,
    )
    .unwrap();
    let out = Command::new("node")
        .args(["--test", "es.test.ts"])
        .current_dir(&target_dir)
        .output()
        .expect("node --test");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "the generated fold does not rebuild as it claims:\n{printed}"
    );
    assert!(printed.contains("pass 10"), "{printed}");
}

/// The event sourcing and CQRS rules: each one blocks a way of having a
/// stream that is not a stream, or a view that lies.
#[test]
fn the_event_sourcing_rules_block() {
    let dir = std::env::temp_dir().join("axon-es-reglas");
    let run = |manifest: &str, ddl: &str| -> String {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sql")).unwrap();
        std::fs::write(dir.join("sql/001_init.expand.sql"), ddl).unwrap();
        std::fs::write(dir.join("libro.toml"), manifest).unwrap();
        let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        assert!(!ok, "paso limpio");
        err
    };

    // the UNIQUE that makes optimistic versioning possible. The whole block is
    // removed so as not to leave an orphan comma: a DDL that does not parse
    // would make this test pass for the wrong reason.
    let without_unique = DDL_ES.replace(
        "  en         timestamptz NOT NULL DEFAULT now(),\n  \
         -- Without this UNIQUE, two concurrent writes on the same stream both get in\n  \
         -- with the same version and nobody sees an error.\n  \
         UNIQUE (stream_id, version)\n",
        "  en         timestamptz NOT NULL DEFAULT now()\n",
    );
    assert!(
        !without_unique.contains("UNIQUE"),
        "the variant did not remove the UNIQUE"
    );
    let err = run(MANIFIESTO_ES, &without_unique);
    assert!(
        err.contains("has no UNIQUE on (stream_id, version)"),
        "{err}"
    );
    assert!(
        err.contains("depends on what order they are read in"),
        "{err}"
    );

    // an aggregate founded on an event the service does not emit
    let foreign = MANIFIESTO_ES.replace(
        r#"events = ["cuenta.abierta@v1", "cuenta.depositada@v1", "cuenta.cerrada@v1"]"#,
        r#"events = ["cuenta.abierta@v1", "other.cosa@v1"]"#,
    );
    let err = run(&foreign, DDL_ES);
    assert!(err.contains("does not declare it emits"), "{err}");
    assert!(err.contains("this is a view, not an aggregate"), "{err}");

    // the view with nowhere to record how far it got
    let without_checkpoint = DDL_ES.replace(
        "CREATE TABLE view_saldos_checkpoint",
        "CREATE TABLE otra_tabla",
    );
    let err = run(MANIFIESTO_ES, &without_checkpoint);
    assert!(err.contains("view_saldos_checkpoint"), "{err}");
    assert!(err.contains("reprocesses from the beginning"), "{err}");

    // a view staler than the service's budget
    let stale = MANIFIESTO_ES.replace("max_staleness_ms = 3000", "max_staleness_ms = 9000");
    let err = run(&stale, DDL_ES);
    assert!(err.contains("allows 9000ms of lag"), "{err}");
    assert!(err.contains("cannot honour what it promised"), "{err}");

    // and a view under `consistency = "strong"`
    let strong = MANIFIESTO_ES
        .replace("consistency = \"eventual\"", "consistency = \"strong\"")
        .replace("max_staleness_ms = 5000\n", "");
    let err = run(&strong, DDL_ES);
    assert!(err.contains("stale by definition"), "{err}");

    // the shadow that does not match the view
    let short_shadow = DDL_ES.replace(
        "CREATE TABLE view_saldos_shadow (\n  stream_id  uuid PRIMARY KEY,\n  centavos   bigint NOT NULL,\n  posicion   bigint NOT NULL\n);",
        "CREATE TABLE view_saldos_shadow (\n  stream_id  uuid PRIMARY KEY,\n  posicion   bigint NOT NULL\n);",
    );
    // the guard compares against the original: the same text is also in the
    // live view, so searching for it loose does not say whether the replace applied
    assert_ne!(short_shadow, DDL_ES, "the variant did not apply");
    let err = run(MANIFIESTO_ES, &short_shadow);
    assert!(
        err.contains("`view_saldos_shadow` has no `centavos` column"),
        "{err}"
    );
    assert!(err.contains("only then would it show"), "{err}");

    // and with no shadow: rebuilding in place serves an incomplete view
    let without_shadow = DDL_ES.replace(
        "CREATE TABLE view_saldos_shadow",
        "CREATE TABLE other_shadow",
    );
    let err = run(MANIFIESTO_ES, &without_shadow);
    assert!(err.contains("`view_saldos_shadow` is missing"), "{err}");
    assert!(err.contains("fewer rows than there are"), "{err}");

    // the view's checkpoint, with no stream in the key: one stream overwrites the other
    let cp_global = DDL_ES.replace(
        "  view_name  text NOT NULL,\n  -- per STREAM: an event's version is its position inside ITS stream, so a\n  -- single number for the whole view identifies nothing as soon as there is\n  -- more than one stream\n  stream_id  uuid NOT NULL,\n  position   bigint NOT NULL,\n  PRIMARY KEY (view_name, stream_id)\n",
        "  view_name  text PRIMARY KEY,\n  stream_id  uuid NOT NULL,\n  position   bigint NOT NULL\n",
    );
    assert!(
        !cp_global.contains("PRIMARY KEY (view_name, stream_id)"),
        "the variant did not apply"
    );
    let err = run(MANIFIESTO_ES, &cp_global);
    assert!(
        err.contains("has no key on (view_name, stream_id)"),
        "{err}"
    );
    assert!(err.contains("One stream would overwrite another"), "{err}");

    // snapshots declared with no table, and without the column that makes them safe
    let without_table = DDL_ES.replace("CREATE TABLE cuenta_snapshot", "CREATE TABLE otra_foto");
    let err = run(MANIFIESTO_ES, &without_table);
    assert!(err.contains("with no `cuenta_snapshot` table"), "{err}");

    let without_rules = DDL_ES.replace("  rules     int  NOT NULL,\n", "");
    assert_ne!(without_rules, DDL_ES, "the variant did not apply");
    let err = run(MANIFIESTO_ES, &without_rules);
    assert!(err.contains("has no `rules` column"), "{err}");
    assert!(
        err.contains("no longer matches replaying the stream"),
        "{err}"
    );

    // one snapshot per event is not a cache
    let cada_uno = MANIFIESTO_ES.replace("snapshot_every = 2", "snapshot_every = 1");
    let (out, _, _) = {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sql")).unwrap();
        std::fs::write(dir.join("sql/001_init.expand.sql"), DDL_ES).unwrap();
        std::fs::write(dir.join("libro.toml"), &cada_uno).unwrap();
        axon(&["verify", dir.to_str().unwrap()])
    };
    assert!(
        out.contains("a second copy of the stream"),
        "one snapshot per event raised no warning:\n{out}"
    );

    // an aggregate publishing with no outbox: the dual-write the stream avoided
    let without_outbox = MANIFIESTO_ES.replace("outbox = true", "outbox = false");
    assert!(
        without_outbox.contains("outbox = false"),
        "the variant did not apply"
    );
    let err = run(&without_outbox, DDL_ES);
    assert!(err.contains("needs `[patterns] outbox = true`"), "{err}");
    assert!(
        err.contains("the event is recorded and nobody received it"),
        "{err}"
    );

    // the stream is append-only, and not as a recommendation
    let err = run(
        MANIFIESTO_ES,
        &format!("{DDL_ES}\nUPDATE cuenta_event SET data = '{{}}'::jsonb WHERE version = 1;\n"),
    );
    assert!(err.contains("is the stream of `cuenta`"), "{err}");
    assert!(err.contains("a past that did not happen"), "{err}");
    // and not even marked as `.contract.sql`: there is no permission for that
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
    assert!(
        !ok,
        "a DELETE on the stream got through by living in a .contract.sql"
    );
    assert!(err.contains("is the stream of `cuenta`"), "{err}");

    // an aggregate event no machine transition emits
    let maquina = MANIFIESTO_ES.replace(
        "[aggregate.cuenta]",
        "[machine.cuenta]\ninitial = \"nueva\"\nfinal = [\"cerrada\"]\n\n\
         [machine.cuenta.transitions.abrir]\nfrom = [\"nueva\"]\nto = \"abierta\"\n\
         on = \"cuenta.abierta@v1\"\nemits = \"cuenta.abierta@v1\"\n\n\
         [aggregate.cuenta]\nmachine = \"cuenta\"",
    );
    let err = run(&maquina, DDL_ES);
    assert!(err.contains("no transition of `cuenta` emits it"), "{err}");
    assert!(
        err.contains("would not know which state to take it to"),
        "{err}"
    );
}

/// The gap this closes: the warehouse schema was generated for three dialects
/// and only GCP had an ingest path. The schema could be applied on Snowflake,
/// deployed, and left with empty tables without a single error —which is
/// indistinguishable from "nothing happened in the business".
#[test]
fn ingest_is_not_promised_without_a_path() {
    // Every wired combination has to render, and the resource that carries the
    // events has to be there. A target that "renders" without the resource is
    // the same silence in another shape.
    for (target, warehouse, marker) in [
        ("gcp", "bigquery", "bigquery_config"),
        ("aws", "clickhouse", "aws_kinesis_firehose_delivery_stream"),
        ("aws", "snowflake", "aws_kinesis_firehose_delivery_stream"),
        ("local", "clickhouse", "clickhouse/clickhouse-server"),
        ("k8s", "clickhouse", "image: timberio/vector"),
    ] {
        let f = tuned(warehouse);
        let (out, err, ok) = axon(&["infra", &f, "--target", target]);
        assert!(ok, "{target}+{warehouse}: {err}");
        assert!(
            out.contains(marker),
            "{target}+{warehouse} renders with no `{marker}`: nothing would carry the events to the warehouse"
        );
    }
    // And what has no path is REFUSED, with the name of the combination and
    // what to do about it.
    for (target, warehouse) in [
        ("gcp", "clickhouse"),
        ("aws", "bigquery"),
        ("k8s", "bigquery"),
    ] {
        let f = tuned(warehouse);
        let (_, err, ok) = axon(&["infra", &f, "--target", target]);
        assert!(!ok, "{target}+{warehouse} rendered with no ingest path");
        assert!(err.contains("has no ingest path"), "{err}");
        assert!(err.contains("the tables would stay empty"), "{err}");
        assert!(
            err.contains("export = false"),
            "it does not say what to do:\n{err}"
        );
    }
    // The local loader comes from the same place as the schema: if the JSON's
    // paths did not match the columns, the load would fail at the warehouse
    // and not here.
    let (sql, err, ok) = axon(&["analytics", "examples", "--load", "local.ndjson"]);
    assert!(ok, "{err}");
    assert!(sql.contains("INSERT INTO axon.order_placed_v1"), "{sql}");
    // the hash comes out salted from a parameter, never the value
    assert!(sql.contains("SHA256(concat({salt:String}"), "{sql}");
    assert!(
        !sql.contains("AS customer_email,"),
        "the address travels in plaintext:\n{sql}"
    );
    // idempotent: a periodic loader runs many times over the same log
    assert!(sql.contains("NOT IN (SELECT event_id FROM"), "{sql}");
}

/// Warehouse drift raises an error nowhere: a new field the table does not
/// have loads as nothing, and an old column keeps the data it had. Both give
/// queries that return numbers.
#[test]
fn warehouse_drift_is_detected() {
    let dir = std::env::temp_dir().join("axon-bodega-check");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let tsv = dir.join("real.tsv");

    // The dump matching what is declared, built by hand from the generated
    // schema: if this did not give 0, the rest of the test would say nothing.
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
    let manifest = r#"service = "tienda"
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
    // `pii` goes as a service field, not inside emits
    let manifest = manifest.replace("pii = []\n", "");
    let manifest = manifest.replace("tier = \"1\"", "tier = \"1\"\npii = [\"customerEmail\"]");
    std::fs::write(dir.join("tienda.toml"), &manifest).unwrap();
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

    let (printed, ok) = check(real);
    assert!(ok, "the matching dump reported differences:\n{printed}");
    assert!(printed.contains("0 differences"), "{printed}");

    // a declared column the table does not have
    let missing = real.replace("order_placed_v1\ttotal_amount\tNullable(Int64)\n", "");
    let (printed, ok) = check(&missing);
    assert!(!ok, "a missing column passed clean");
    assert!(
        printed.contains("`order_placed_v1.total_amount` is missing"),
        "{printed}"
    );
    assert!(
        printed.contains("that field is stored nowhere"),
        "{printed}"
    );

    // a type that is not the same: a date stored as text sorts wrong
    let kind = real.replace(
        "order_placed_v1\tevent_time\tDateTime64(3)",
        "order_placed_v1\tevent_time\tString",
    );
    let (printed, ok) = check(&kind);
    assert!(!ok, "a date as text passed clean");
    assert!(printed.contains("is text in the warehouse"), "{printed}");

    // the plaintext address next to the hash: the manifest says `hash` and the
    // old value is still there
    let claro = format!("{real}order_placed_v1\tcustomer_email\tNullable(String)\n");
    let (printed, ok) = check(&claro);
    assert!(!ok, "the plaintext address passed clean");
    assert!(printed.contains("exists in plaintext"), "{printed}");
    assert!(
        printed.contains("keeps the addresses it already had"),
        "{printed}"
    );

    // an extra column is a warning, not an error: it breaks nothing
    let sobra = format!("{real}order_placed_v1\tsobra\tString\n");
    let (printed, ok) = check(&sobra);
    assert!(ok, "an extra column blocked:\n{printed}");
    assert!(
        printed.contains("Left over from an earlier version"),
        "{printed}"
    );

    // And what matters most: an empty dump CANNOT give 0 differences. That is
    // the result of running the query against the wrong warehouse, and reading
    // it as "all fine" is worse than not checking.
    let (printed, ok) = check("");
    assert!(!ok, "an empty dump reported 0 differences");
    assert!(printed.contains("has no columns at all"), "{printed}");
    assert!(
        printed.contains("reads as everything being fine"),
        "{printed}"
    );
}

/// The Vector config is validated with `vector validate`: the parser that
/// will read it is the one that says whether it is right. A `route` with a
/// branch and no consumer, or a wrong field, show up there and not the day an
/// event goes missing from the warehouse.
#[test]
fn the_vector_config_validates() {
    if !has("docker") {
        eprintln!("salteado: falta docker");
        return;
    }
    let dir = std::env::temp_dir().join("axon-vector");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (cfg, err, ok) = axon(&["analytics", "examples", "--vector"]);
    assert!(ok, "{err}");
    std::fs::write(dir.join("vector.yaml"), &cfg).unwrap();

    // One source per event, not a wildcard with a router: a router leaves an
    // `_unmatched` branch, and the event that lands there is dropped in silence.
    assert!(
        !cfg.contains("type: route"),
        "a router leaves events with no consumer"
    );
    assert!(
        cfg.contains("queue: axon-warehouse"),
        "with no queue group, every replica writes the same row"
    );
    // The hash's salt comes in through a variable, never in the generated file.
    assert!(cfg.contains("get_env_var!(\"AXON_PII_SALT\")"), "{cfg}");
    assert!(
        !cfg.contains("customer_email\":"),
        "the address travels in plaintext:\n{cfg}"
    );
    // A field the table does not have is an error, not something to drop.
    assert!(cfg.contains("skip_unknown_fields: false"), "{cfg}");
    // The buffer on disk: in memory, a restart loses what was not written.
    assert!(cfg.contains("type: disk"), "{cfg}");

    let out = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-e",
            "AXON_WAREHOUSE_USER=u",
            "-e",
            "AXON_WAREHOUSE_PASSWORD=p",
            "-e",
            "AXON_PII_SALT=s",
            "-v",
            &format!("{}:/etc/vector:ro", dir.display()),
            "timberio/vector:0.44.0-alpine",
            // `--no-environment` does not check connections: there is no broker
            // and no warehouse here, and what is validated is the config, not the environment.
            "validate",
            "--no-environment",
            "/etc/vector/vector.yaml",
        ])
        .output()
        .expect("docker run vector");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if printed.contains("Unable to find image")
        || printed.contains("Cannot connect to the Docker daemon")
    {
        eprintln!("skipped: the vector image is missing");
        return;
    }
    assert!(
        out.status.success(),
        "vector does not validate the generated config:\n{printed}"
    );
    // a warning today is a lost event tomorrow
    assert!(
        !printed.contains("warning"),
        "it validates with warnings:\n{printed}"
    );
    assert!(
        !printed.contains("no consumers"),
        "a branch with no consumer:\n{printed}"
    );
}

/// The failure rules refute. A wrong declaration is worse than none: the
/// generated client stops retrying what the manifest calls final, so a `409`
/// marked retriable would silence a retry that would have worked.
#[test]
fn the_failure_rules_block() {
    let dir = std::env::temp_dir().join("axon-errors");
    let base = r#"service = "shop"
version = "1.0.0"
owner = "team"
tier = "1"
"#;
    let run = |extra: &str| -> String {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shop.toml"), format!("{base}{extra}")).unwrap();
        let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        assert!(!ok, "it passed clean:\n{extra}");
        err
    };
    let method = |errors: &str| {
        format!(
            "[methods.pay]\nhttp = \"POST /v1/pays\"\nauth = \"required\"\nidempotent = true\n\
             in = {{ id = \"uuid\" }}\nout = {{ id = \"uuid\" }}\nerrors = [{errors}]\n"
        )
    };

    // a 2xx is not a failure
    let err = run(&method("{ code = \"declined\", status = 200 }"));
    assert!(err.contains("is not a failure"), "{err}");

    // the code travels on the wire and gets compared as a literal
    let err = run(&method("{ code = \"Card Declined\", status = 402 }"));
    assert!(err.contains("is not snake_case"), "{err}");

    // the same code twice: which status wins would depend on the order
    let err = run(&method(
        "{ code = \"declined\", status = 402 }, { code = \"declined\", status = 409 }",
    ));
    assert!(err.contains("declares `declined` twice"), "{err}");

    // and the one that matters: a 4xx says the request is what is wrong, so
    // retrying it ends the same way. 408, 425 and 429 are the exceptions.
    let err = run(&method(
        "{ code = \"declined\", status = 402, retriable = true }",
    ));
    assert!(err.contains("retriable = true` on 402"), "{err}");
    assert!(err.contains("ends the same"), "{err}");

    // the three that mean "not yet" DO pass, and so does a 5xx
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("shop.toml"),
        format!(
            "{base}{}",
            method(
                "{ code = \"too_many\", status = 429, retriable = true }, \
                 { code = \"issuer_down\", status = 503, retriable = true }"
            )
        ),
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(
        ok,
        "a retriable 429 and a retriable 503 were rejected:\n{err}"
    );
}

/// `retriable` is not documentation: it lands in the generated client and
/// decides whether it tries again. This is the projection, checked on the
/// example —where `payoutMerchant` declares one final failure and one that is
/// not— and the demo measures the same thing against the running containers.
#[test]
fn the_generated_client_only_retries_what_is_declared_retriable() {
    let (ts, err, ok) = axon(&["build", "examples/checkout.toml", "examples"]);
    assert!(ok, "{err}");
    // the retriable one travels, the final one does not
    assert!(
        ts.contains("(code) => [\"rail_busy\"].includes(code)"),
        "the client does not carry the retriable codes:\n{ts}"
    );
    assert!(
        !ts.contains("\"merchant_ceiling\"].includes(code)"),
        "a failure declared as final came out as retriable"
    );
    // and the loop that uses it: without this line the list decides nothing
    assert!(
        ts.contains("if (err instanceof AxonProblem && !retriable(err.code)) throw err;"),
        "withPolicy does not consult the declared codes"
    );

    // the server side: `fail` typed against the manifest, so a code that is not
    // declared does not compile
    let (ts, err, ok) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(ok, "{err}");
    assert!(
        ts.contains("export function fail<M extends keyof Declared>"),
        "{ts}"
    );
    assert!(
        ts.contains("{ code: \"merchant_ceiling\", status: 409, retriable: false"),
        "the declared table does not carry the manifest's failure"
    );

    // and the OpenAPI says the same thing: one response per declared code
    let (api, err, ok) = axon(&["openapi", "examples"]);
    assert!(ok, "{err}");
    let v: serde_json::Value = serde_json::from_str(&api).unwrap();
    let payout = &v["paths"]["/v1/payouts"]["post"]["responses"];
    assert_eq!(payout["409"]["x-axon-code"], "merchant_ceiling", "{payout}");
    assert_eq!(payout["409"]["x-axon-retriable"], false, "{payout}");
    assert_eq!(payout["503"]["x-axon-retriable"], true, "{payout}");
}

/// The rules for a retired version refute. What makes them worth having is
/// that a deprecation announced in a chat thread is not a deprecation: it has
/// to have a date, a successor, and somebody able to say who is still calling.
#[test]
fn the_retirement_rules_block() {
    let dir = std::env::temp_dir().join("axon-sunset");
    let base = r#"service = "shop"
version = "1.0.0"
owner = "team"
tier = "1"
"#;
    let method = |extra: &str| {
        format!(
            "[methods.read]\nhttp = \"GET /v1/things\"\nauth = \"required\"\n\
             in = {{ id = \"uuid\" }}\nout = {{ id = \"uuid\" }}\n{extra}"
        )
    };
    let run = |extra: &str| -> String {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shop.toml"), format!("{base}{}", method(extra))).unwrap();
        let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        assert!(!ok, "it passed clean:\n{extra}");
        err
    };

    // a version that dies without ever having been marked as dying
    let err = run("sunset = \"2027-01-01\"\n");
    assert!(err.contains("has `sunset` and no `deprecated`"), "{err}");
    assert!(err.contains("the day it stops answering"), "{err}");

    // no window to migrate in
    let err = run("deprecated = \"2027-06-01\"\nsunset = \"2027-01-01\"\n");
    assert!(err.contains("there is no window to migrate in"), "{err}");

    // past its own date and still declared, the same criterion as an expired flag
    let err = run("deprecated = \"2020-01-01\"\nsunset = \"2020-06-01\"\n");
    assert!(err.contains("sunset on 2020-06-01"), "{err}");
    assert!(err.contains("renewed as a decision"), "{err}");

    // a date that is not a date, and a successor that is not a method
    let err = run("deprecated = \"soon\"\n");
    assert!(err.contains("is not in YYYY-MM-DD form"), "{err}");
    let err = run("deprecated = \"2026-01-01\"\nsuccessor = \"readV2\"\n");
    assert!(err.contains("is not a method of the service"), "{err}");
    let err = run("deprecated = \"2026-01-01\"\nsuccessor = \"read\"\n");
    assert!(err.contains("its own `successor`"), "{err}");

    // and the one only a platform-wide view can answer: who still calls it.
    // Inside one repo this is a grep; across twenty services it is the question
    // nobody can answer.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("shop.toml"),
        format!(
            "{base}{}",
            method(
                "deprecated = \"2026-01-01\"\nsunset = \"2027-01-01\"\nsuccessor = \"readV2\"\n\
                    [methods.readV2]\nhttp = \"GET /v2/things\"\nauth = \"required\"\n\
                    in = { id = \"uuid\" }\nout = { id = \"uuid\" }\n"
            )
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("front.toml"),
        "service = \"front\"\nversion = \"1.0.0\"\nowner = \"team\"\ntier = \"1\"\n\n\
         [[depends]]\nservice = \"shop\"\nmethod = \"read\"\ntimeout_ms = 1000\n",
    )
    .unwrap();
    // a warning is not an error: it comes out on stdout and does not fail CI on
    // its own. Naming who calls it is the point — deciding is the team's
    let (out, _, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "a deprecated dependency blocked the build");
    assert!(
        out.contains("front calls shop.read, which is deprecated and sunsets on 2027-01-01"),
        "{out}"
    );
    assert!(out.contains("the successor is `readV2`"), "{out}");
}

/// The retirement is projected onto the three places a caller can find out
/// from: the headers of the response, the caller's own client, and the
/// published document. The demo measures the first one against containers.
#[test]
fn the_retirement_travels_where_the_caller_looks() {
    // the headers, with the formats each RFC asks for and not the manifest's
    // ISO date, which a client could not parse
    let (ts, err, ok) = axon(&["build", "examples/orders.toml", "examples"]);
    assert!(ok, "{err}");
    assert!(
        ts.contains("\"sunset\": \"Fri, 31 Dec 2027 00:00:00 GMT\""),
        "the Sunset is not an HTTP-date:\n{ts}"
    );
    assert!(
        ts.contains("\"deprecation\": \"@1788220800\""),
        "the Deprecation is not an sf-date"
    );
    assert!(
        ts.contains("rel=\\\"successor-version\\\"") && ts.contains("/v2/tenants/"),
        "the successor does not travel as a Link"
    );
    // the current version announces nothing: there is nothing to announce
    assert!(
        !ts.contains("\"GET /v2/tenants/{tenantId}/orders/{orderId}\": {"),
        "the successor came out marked as retired too"
    );

    // the caller's client: `@deprecated` is read by the editor and by review
    let (ts, err, ok) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(ok, "{err}");
    assert!(
        ts.contains("@deprecated orders.getOrder is deprecated; it sunsets on 2027-12-31"),
        "the client does not say that what it calls is dying:\n{ts}"
    );

    // and the published document
    let (api, err, ok) = axon(&["openapi", "examples"]);
    assert!(ok, "{err}");
    let v: serde_json::Value = serde_json::from_str(&api).unwrap();
    let v1 = &v["paths"]["/v1/tenants/{tenantId}/orders/{orderId}"]["get"];
    assert_eq!(v1["deprecated"], true, "{v1}");
    assert_eq!(v1["x-axon-sunset"], "2027-12-31", "{v1}");
    assert_eq!(
        v1["x-axon-successor"], "/v2/tenants/{tenantId}/orders/{orderId}",
        "{v1}"
    );
    let v2 = &v["paths"]["/v2/tenants/{tenantId}/orders/{orderId}"]["get"];
    assert!(
        v2["deprecated"].is_null(),
        "the v2 came out deprecated: {v2}"
    );
}

/// The dated-version scheme, generated and RUN.
///
/// Unlike the rest of the demo this is not measured against containers: the
/// example uses the path scheme, so what runs here is `tsc --strict` plus
/// `node --test` over the emitted chain. It is still the real tool of the
/// ecosystem and not axon's own asserts.
#[test]
fn the_version_adapters_chain_in_the_right_order() {
    if !has("node") {
        eprintln!("salteado: node no esta instalado");
        return;
    }
    let dir = std::env::temp_dir().join("axon-apiver");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("shop.toml"),
        r#"service = "shop"
version = "1.0.0"
owner = "team"
tier = "1"

[api]
versioning = "header"
header = "X-Api-Version"
default = "2026-09-01"
support_window_days = 365
lts_window_days = 1095

[[api.version]]
date = "2026-01-15"
lts = true
sunset = "2029-01-15"

[[api.version]]
date = "2026-05-01"
deprecated = "2026-09-01"
sunset = "2027-06-01"

[[api.version]]
date = "2026-09-01"

[methods.getOrder]
http = "GET /orders/{orderId}"
auth = "required"
timeout_ms = 2000
in = { orderId = "uuid" }
out = { orderId = "uuid", status = "string", customer = "json" }

[methods.getOrder.at."2026-05-01"]
out = { orderId = "uuid", status = "string", customerId = "uuid" }
adapter = "downgradeTo202605"

[methods.getOrder.at."2026-01-15"]
out = { orderId = "uuid", state = "string", customerId = "uuid" }
adapter = "downgradeTo202601"
"#,
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "the header scheme does not verify clean:\n{err}");

    let (ts, err, ok) = axon(&[
        "build",
        dir.join("shop.toml").to_str().unwrap(),
        dir.to_str().unwrap(),
    ]);
    assert!(ok, "{err}");
    // each adapter receives the shape the one above it produced: that is what
    // makes the chain a chain and not two independent translations
    assert!(
        ts.contains("response(out: GetOrderOutAt20260501): GetOrderOutAt20260115;"),
        "the chain is not typed step by step:\n{ts}"
    );
    std::fs::write(dir.join("contracts.ts"), &ts).unwrap();

    // The person's side, which is the whole point of the scheme: one
    // implementation and N small translations, not N implementations.
    //
    // It goes in its own file with no `node:` imports so `tsc --strict` can
    // check it without @types/node: what has to typecheck is the adapter
    // against the interface the manifest generated.
    std::fs::write(
        dir.join("adapters.ts"),
        r#"import { adaptGetOrder, resolveApiVersion, apiVersionHeaders,
         type Adapters, type GetOrderOut } from "./contracts.ts";

export const adapters: Adapters = {
  downgradeTo202605: {
    response: (out) => ({ orderId: out.orderId, status: out.status,
                          customerId: (out.customer as { id: string }).id }),
  },
  downgradeTo202601: {
    response: (out) => ({ orderId: out.orderId, state: out.status, customerId: out.customerId }),
  },
};

export const answer: GetOrderOut = { orderId: "o1", status: "placed", customer: { id: "c1" } };
export const asOf = (pinned?: string) =>
  adaptGetOrder(resolveApiVersion(pinned), answer, adapters);
export const headersFor = (pinned: string) => apiVersionHeaders(resolveApiVersion(pinned));
export { resolveApiVersion };
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("api.test.ts"),
        r#"import { test } from "node:test";
import assert from "node:assert/strict";
import { asOf, answer, headersFor, resolveApiVersion } from "./adapters.ts";
import { AxonProblem } from "./contracts.ts";

test("an unpinned caller gets the current shape", () => {
  assert.deepEqual(asOf(), answer);
});

test("a pinned version gets the shape that version promised", () => {
  assert.deepEqual(asOf("2026-05-01"), { orderId: "o1", status: "placed", customerId: "c1" });
});

test("the oldest one comes out of the whole chain, in order", () => {
  // TWO adapters applied one after the other: `state` exists only in the
  // oldest shape, and `customerId` only after the first step
  assert.deepEqual(asOf("2026-01-15"), { orderId: "o1", state: "placed", customerId: "c1" });
});

test("an undeclared version is a 400 and not a guess", () => {
  try {
    resolveApiVersion("2020-01-01");
    assert.fail("it accepted a version nobody declared");
  } catch (e) {
    assert.ok(e instanceof AxonProblem);
    assert.equal(e.status, 400);
    assert.equal(e.code, "unknown_api_version");
  }
});

test("the response says the answer depends on the header, and carries the cycle", () => {
  const h = headersFor("2026-05-01");
  assert.equal(h["vary"], "X-Api-Version");
  assert.equal(h["x-api-version"], "2026-05-01");
  assert.equal(h["sunset"], "Tue, 01 Jun 2027 00:00:00 GMT");
  assert.ok(h["deprecation"].startsWith("@"));
});
"#,
    )
    .unwrap();

    let tsc = Command::new("npx")
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
            "--module",
            "nodenext",
            "--moduleResolution",
            "nodenext",
            "--allowImportingTsExtensions",
            "adapters.ts",
        ])
        .current_dir(&dir)
        .output()
        .expect("npx");
    assert!(
        tsc.status.success(),
        "the adapters do not typecheck:\n{}{}",
        String::from_utf8_lossy(&tsc.stdout),
        String::from_utf8_lossy(&tsc.stderr)
    );

    let out = Command::new("node")
        .args(["--test", "api.test.ts"])
        .current_dir(&dir)
        .output()
        .expect("node --test");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "the chain does not run:\n{printed}");
    assert!(printed.contains("in order"), "{printed}");
    assert!(printed.contains("fail 0"), "{printed}");
}

/// The maintenance cycle refutes. The promise "we support a version for a
/// year" is worth what it can be checked with: here it is a window in days,
/// applied to every declared version.
#[test]
fn the_version_cycle_rules_block() {
    let dir = std::env::temp_dir().join("axon-cycle");
    let head = |api: &str| {
        format!(
            "service = \"shop\"\nversion = \"1.0.0\"\nowner = \"team\"\ntier = \"1\"\n\n\
             [api]\nversioning = \"header\"\n{api}\n\
             [methods.read]\nhttp = \"GET /things\"\nauth = \"required\"\n\
             in = {{ id = \"uuid\" }}\nout = {{ id = \"uuid\" }}\n"
        )
    };
    let run = |body: String| -> String {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shop.toml"), body).unwrap();
        let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
        assert!(!ok, "it passed clean");
        err
    };

    // an LTS with no date is not a promise
    let err = run(head(
        "[[api.version]]\ndate = \"2026-01-15\"\nlts = true\n\n\
                        [[api.version]]\ndate = \"2026-09-01\"\n",
    ));
    assert!(err.contains("has no `sunset`"), "{err}");
    assert!(err.contains("it is a hope"), "{err}");

    // the declared window, applied: 90 days of support against a promise of 365
    let err = run(head(
        "support_window_days = 365\n\n\
         [[api.version]]\ndate = \"2026-01-15\"\nsunset = \"2026-04-15\"\n\n\
         [[api.version]]\ndate = \"2026-09-01\"\n",
    ));
    assert!(err.contains("is served for 90 days"), "{err}");
    assert!(err.contains("window is 365"), "{err}");

    // an LTS that dies before the ordinary version that follows it
    let err = run(head(
        "[[api.version]]\ndate = \"2026-01-15\"\nlts = true\nsunset = \"2027-01-15\"\n\n\
         [[api.version]]\ndate = \"2026-05-01\"\nsunset = \"2028-05-01\"\n\n\
         [[api.version]]\ndate = \"2026-09-01\"\n",
    ));
    assert!(err.contains("dies before `2026-05-01`"), "{err}");
    assert!(err.contains("just a label"), "{err}");

    // a version out of order adapts backwards
    let err = run(head(
        "[[api.version]]\ndate = \"2026-09-01\"\n\n[[api.version]]\ndate = \"2026-01-15\"\n",
    ));
    assert!(err.contains("comes after a newer one"), "{err}");

    // and a shape from the past with nobody to translate it, which is the one
    // that decides whether one implementation can serve an old version
    let versions = "[[api.version]]\ndate = \"2026-01-15\"\nsunset = \"2028-01-15\"\n\n\
                    [[api.version]]\ndate = \"2026-09-01\"\n";
    let err = run(format!(
        "{}{versions}[methods.read.at.\"2026-01-15\"]\nout = {{ id = \"uuid\", name = \"string\" }}\n",
        head("")
    ));
    assert!(err.contains("declares no `adapter`"), "{err}");
    assert!(err.contains("What changed: name"), "{err}");

    // a version that changed nothing is an adapter that copies
    let err = run(format!(
        "{}{versions}[methods.read.at.\"2026-01-15\"]\nout = {{ id = \"uuid\" }}\nadapter = \"back\"\n",
        head("")
    ));
    assert!(err.contains("declares the same shape"), "{err}");

    // the two schemes at once: the route versions AND the header versions
    let err = run(format!(
        "service = \"shop\"\nversion = \"1.0.0\"\nowner = \"team\"\ntier = \"1\"\n\n\
         [api]\nversioning = \"header\"\n{versions}\n\
         [methods.read]\nhttp = \"GET /v1/things\"\nauth = \"required\"\n\
         in = {{ id = \"uuid\" }}\nout = {{ id = \"uuid\" }}\n"
    ));
    assert!(
        err.contains("versions the route while `[api]` versions by header"),
        "{err}"
    );

    // and one platform, one scheme: `[api]` cannot differ between services
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("shop.toml"), format!("{}{versions}", head(""))).unwrap();
    std::fs::write(
        dir.join("front.toml"),
        "service = \"front\"\nversion = \"1.0.0\"\nowner = \"team\"\ntier = \"1\"\n",
    )
    .unwrap();
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok, "two schemes at once passed clean:\n{err}");
    assert!(err.contains("declare different `[api]`"), "{err}");
    assert!(err.contains("one decision for the whole platform"), "{err}");
}

/// Declared consumption: which fields each consumer really reads.
#[test]
fn the_declared_consumption_rules_block() {
    let dir = std::env::temp_dir().join("axon-uses");
    let write = |shop_extra: &str, front: &str| {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("shop.toml"),
            format!(
                "service = \"shop\"\nversion = \"1.0.0\"\nowner = \"team\"\ntier = \"1\"\n\n\
                 [analytics]\nexport = false\n\n\
                 [emits.\"order.placed@v1\"]\norderId = \"uuid\"\ntotal = \"money\"\n\n\
                 [methods.getOrder]\nhttp = \"GET /v1/orders/{{orderId}}\"\nauth = \"required\"\n\
                 in = {{ orderId = \"uuid\" }}\nout = {{ orderId = \"uuid\", status = \"string\" }}\n\
                 {shop_extra}"
            ),
        )
        .unwrap();
        std::fs::write(
            dir.join("front.toml"),
            format!(
                "service = \"front\"\nversion = \"1.0.0\"\nowner = \"team\"\ntier = \"1\"\n{front}"
            ),
        )
        .unwrap();
    };

    // a field the provider does not return: either it was renamed and the
    // caller is reading `undefined`, or the declaration is wrong
    write(
        "",
        "[[depends]]\nservice = \"shop\"\nmethod = \"getOrder\"\ntimeout_ms = 1000\n\
         uses = [\"orderId\", \"customer\"]\n",
    );
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok, "{err}");
    assert!(err.contains("uses `shop.getOrder.customer`"), "{err}");
    assert!(err.contains("which shop does not return"), "{err}");

    // the same for an event
    write(
        "",
        "[consumes.\"order.placed@v1\"]\nhandler = \"onPlaced\"\nuses = [\"orderId\", \"tax\"]\n",
    );
    let (_, err, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(!ok, "{err}");
    assert!(err.contains("uses `order.placed@v1.tax`"), "{err}");

    // and the answer nobody has today: a field NOBODY reads. It only comes out
    // when every consumer declared what it uses — with one that declared
    // nothing there is no answer, and saying "delete it" without one is how a
    // field somebody was reading gets deleted.
    write(
        "",
        "[consumes.\"order.placed@v1\"]\nhandler = \"onPlaced\"\nuses = [\"orderId\"]\n\n\
         [[depends]]\nservice = \"shop\"\nmethod = \"getOrder\"\ntimeout_ms = 1000\n\
         uses = [\"orderId\"]\n",
    );
    let (out, _, ok) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(ok, "{out}");
    assert!(
        out.contains("order.placed@v1.total is read by nobody"),
        "{out}"
    );
    assert!(
        out.contains("shop.getOrder returns `status` and no caller reads it"),
        "{out}"
    );

    // with one consumer that declared nothing, it stays quiet
    write(
        "",
        "[consumes.\"order.placed@v1\"]\nhandler = \"onPlaced\"\n\n\
         [[depends]]\nservice = \"shop\"\nmethod = \"getOrder\"\ntimeout_ms = 1000\n",
    );
    let (out, _, _) = axon(&["verify", dir.to_str().unwrap()]);
    assert!(
        !out.contains("read by nobody"),
        "it claimed nobody reads it without an answer:\n{out}"
    );
}

/// The double of the dependencies, and the declared policy exercised with no
/// network. It is the same claim the demo measures against containers — a
/// retriable failure arrives 1 + retries times and a final one exactly once —
/// provable here in a unit test.
#[test]
fn the_generated_double_runs_the_declared_policy() {
    if !has("node") {
        eprintln!("salteado: node no esta instalado");
        return;
    }
    let dir = std::env::temp_dir().join("axon-double");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (ts, err, ok) = axon(&["build", "examples/checkout.toml", "examples"]);
    assert!(ok, "{err}");
    std::fs::write(dir.join("contracts.ts"), ts).unwrap();
    let (kit, err, ok) = axon(&["test", "examples/checkout.toml", "examples"]);
    assert!(ok, "{err}");
    std::fs::write(dir.join("axon.testkit.ts"), kit).unwrap();
    std::fs::write(
        dir.join("double.test.ts"),
        r#"import { test } from "node:test";
import assert from "node:assert/strict";
import { fakeClients } from "./axon.testkit.ts";
import { AxonProblem, newEnvelope } from "./contracts.ts";

const e = newEnvelope("test", "test", {});
const input = { paymentId: "p1", amount: { amount: 10, currency: "MXN" } };

test("the double answers the contract, with no network", async () => {
  const [clients, t] = fakeClients();
  const out = await clients.paymentsCapturePayment({ orderId: "o1", amount: input.amount }, e);
  assert.equal(typeof out.paymentId, "string");
  assert.equal(t.timesCalled("payments", "capturePayment"), 1);
});

test("a retriable failure spends the whole declared budget", async () => {
  const [clients, t] = fakeClients();
  t.failWith("payments", "payoutMerchant", new AxonProblem(503, "rail_busy"));
  await assert.rejects(() => clients.paymentsPayoutMerchant(input, e));
  // 2 declared retries: 1 + 2
  assert.equal(t.timesCalled("payments", "payoutMerchant"), 3);
});

test("a final failure arrives exactly once", async () => {
  const [clients, t] = fakeClients();
  t.failWith("payments", "payoutMerchant", new AxonProblem(409, "merchant_ceiling"));
  await assert.rejects(() => clients.paymentsPayoutMerchant(input, e));
  assert.equal(t.timesCalled("payments", "payoutMerchant"), 1);
});

test("what it was asked is what the caller sent", async () => {
  const [clients, t] = fakeClients();
  await clients.paymentsPayoutMerchant(input, e);
  assert.deepEqual(t.calls[0], { target: "payments", method: "payoutMerchant", body: input });
});
"#,
    )
    .unwrap();
    let out = Command::new("node")
        .args(["--test", "double.test.ts"])
        .current_dir(&dir)
        .output()
        .expect("node --test");
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "the double does not run:\n{printed}");
    assert!(
        printed.contains("spends the whole declared budget"),
        "{printed}"
    );
    assert!(printed.contains("fail 0"), "{printed}");
}

/// And the part that keeps `uses` from lying: the field nobody declared does
/// not exist on this side, so reading it does not compile. A Pact recorded once
/// goes stale the day somebody reads one more field; this cannot.
#[test]
fn reading_an_undeclared_field_does_not_compile() {
    if !has("node") {
        eprintln!("salteado: node no esta instalado");
        return;
    }
    let dir = std::env::temp_dir().join("axon-undeclared");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (ts, err, ok) = axon(&["build", "examples/payments.toml", "examples"]);
    assert!(ok, "{err}");
    std::fs::write(dir.join("contracts.ts"), ts).unwrap();
    // `orders` declares customerEmail on the event; payments declared it reads
    // orderId and total
    std::fs::write(
        dir.join("read.ts"),
        "import { type OrderPlacedV1 } from \"./contracts.ts\";\n\
         export const declared = (d: OrderPlacedV1) => d.orderId;\n\
         export const undeclared = (d: OrderPlacedV1) => d.customerEmail;\n",
    )
    .unwrap();
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
            "--module",
            "nodenext",
            "--moduleResolution",
            "nodenext",
            "--allowImportingTsExtensions",
            "read.ts",
        ])
        .current_dir(&dir)
        .output()
        .expect("npx");
    let printed = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        !out.status.success(),
        "reading a field nobody declared compiled:\n{printed}"
    );
    assert!(
        printed.contains("customerEmail") && printed.contains("does not exist"),
        "{printed}"
    );
    // and the declared one is not what broke it
    assert!(!printed.contains("orderId"), "{printed}");
}
