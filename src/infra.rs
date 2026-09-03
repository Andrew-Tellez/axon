//! IaC agnostica: el manifiesto produce un PLAN neutral, y cada target lo
//! renderiza. Un proveedor sin target propio se resuelve con `axon plan`,
//! que emite el JSON del plan para que lo rendericen ustedes.
use crate::manifest::*;
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Serialize)]
pub struct Topic {
    pub event: String,
    pub name: String,
    pub dlq: String,
}

#[derive(Debug, Serialize)]
pub struct Sub {
    pub service: String,
    pub event: String,
    pub name: String,
    pub max_attempts: u32,
}

#[derive(Debug, Serialize)]
pub struct Store {
    pub service: String,
    pub engine: String,
    pub outbox: bool,
}

#[derive(Debug, Serialize)]
pub struct Secret {
    pub service: String,
    pub key: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct Workload {
    pub service: String,
    pub min_instances: u32,
    pub subscribes: Vec<String>,
}

/// Plan neutral. Sin una sola palabra de ningun proveedor.
#[derive(Debug, Serialize)]
pub struct Plan {
    pub topics: Vec<Topic>,
    pub subs: Vec<Sub>,
    pub stores: Vec<Store>,
    pub secrets: Vec<Secret>,
    pub workloads: Vec<Workload>,
}

pub fn plan(ms: &[Manifest]) -> Plan {
    let events: BTreeSet<&String> = ms.iter().flat_map(|m| m.emits.keys()).collect();
    let topics = events
        .iter()
        .map(|ev| Topic {
            event: ev.to_string(),
            name: topic(ev),
            dlq: format!("{}.dlq", topic(ev)),
        })
        .collect();

    let mut subs = Vec::new();
    let mut stores = Vec::new();
    let mut secrets = Vec::new();
    let mut workloads = Vec::new();
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        for ev in m.consumes.keys() {
            subs.push(Sub {
                service: svc.clone(),
                event: ev.clone(),
                name: format!("{svc}--{}", topic(ev)),
                // DLQ siempre: no hay forma de declarar un consumidor sin ella
                max_attempts: 5,
            });
        }
        if let Some(engine) = &m.infra.state {
            stores.push(Store {
                service: svc.clone(),
                engine: engine.clone(),
                outbox: m.patterns.outbox,
            });
        }
        for k in &m.infra.secrets {
            secrets.push(Secret {
                service: svc.clone(),
                key: k.clone(),
                name: format!("{svc}-{}", k.to_lowercase().replace('_', "-")),
            });
        }
        workloads.push(Workload {
            service: svc.clone(),
            min_instances: m.infra.min_instances.unwrap_or(0),
            subscribes: m.consumes.keys().cloned().collect(),
        });
    }
    Plan {
        topics,
        subs,
        stores,
        secrets,
        workloads,
    }
}

pub const NATIVE: [&str; 5] = ["local", "gcp", "aws", "k8s", "plan"];

pub fn render(p: &Plan, target: &str) -> Result<String, String> {
    match target {
        "gcp" => Ok(gcp(p)),
        "aws" => Ok(aws(p)),
        "k8s" => Ok(k8s(p)),
        "local" => Ok(local(p)),
        "plan" => serde_json::to_string_pretty(p).map_err(|e| e.to_string()),
        other => Err(format!(
            "target `{other}` desconocido. Nativos: local, gcp, aws, k8s. \
             Para cualquier otro: `axon infra --target plan` da el plan neutral en JSON \
             y lo renderizas con tu propia plantilla."
        )),
    }
}

const HEAD: &str = "# generado por axon — no editar\n";

fn gcp(p: &Plan) -> String {
    let mut o = vec![HEAD.to_string()];
    for t in &p.topics {
        let n = tfname(&t.event);
        o.push(format!(
            "resource \"google_pubsub_topic\" \"{n}\" {{\n  name = \"{}\"\n}}\n",
            t.name
        ));
        o.push(format!(
            "resource \"google_pubsub_topic\" \"{n}_dlq\" {{\n  name = \"{}\"\n}}\n",
            t.dlq
        ));
    }
    for s in &p.subs {
        let n = tfname(&s.event);
        o.push(format!(
            "resource \"google_pubsub_subscription\" \"{}_{n}\" {{\n  name  = \"{}\"\n  \
             topic = google_pubsub_topic.{n}.name\n  dead_letter_policy {{\n    \
             dead_letter_topic     = google_pubsub_topic.{n}_dlq.id\n    \
             max_delivery_attempts = {}\n  }}\n}}\n",
            tfname(&s.service),
            s.name,
            s.max_attempts
        ));
    }
    for s in &p.stores {
        o.push(format!(
            "resource \"google_sql_database\" \"{}\" {{\n  name     = \"{}\"\n  instance = var.sql_instance\n}}\n",
            tfname(&s.service), s.service
        ));
        if s.outbox {
            o.push(format!(
                "resource \"google_sql_user\" \"{}_relay\" {{\n  name     = \"{}-outbox-relay\"\n  instance = var.sql_instance\n}}\n",
                tfname(&s.service), s.service
            ));
        }
    }
    for s in &p.secrets {
        o.push(format!(
            "resource \"google_secret_manager_secret\" \"{}_{}\" {{\n  secret_id = \"{}\"\n  replication {{ auto {{}} }}\n}}\n",
            tfname(&s.service), tfname(&s.key), s.name
        ));
    }
    o.join("\n")
}

fn aws(p: &Plan) -> String {
    let mut o = vec![HEAD.to_string()];
    for t in &p.topics {
        let n = tfname(&t.event);
        o.push(format!(
            "resource \"aws_sns_topic\" \"{n}\" {{\n  name = \"{}\"\n}}\n",
            tfname(&t.name)
        ));
    }
    for s in &p.subs {
        let (n, q) = (tfname(&s.event), tfname(&s.name));
        o.push(format!(
            "resource \"aws_sqs_queue\" \"{q}_dlq\" {{\n  name = \"{q}-dlq\"\n}}\n"
        ));
        o.push(format!(
            "resource \"aws_sqs_queue\" \"{q}\" {{\n  name = \"{q}\"\n  redrive_policy = jsonencode({{\n    \
             deadLetterTargetArn = aws_sqs_queue.{q}_dlq.arn\n    maxReceiveCount = {}\n  }})\n}}\n",
            s.max_attempts
        ));
        o.push(format!(
            "resource \"aws_sns_topic_subscription\" \"{q}\" {{\n  topic_arn = aws_sns_topic.{n}.arn\n  \
             protocol  = \"sqs\"\n  endpoint  = aws_sqs_queue.{q}.arn\n}}\n"
        ));
    }
    for s in &p.stores {
        o.push(format!(
            "resource \"aws_db_instance\" \"{}\" {{\n  identifier     = \"{}\"\n  engine         = \"{}\"\n  \
             instance_class = var.db_instance_class\n  allocated_storage = 20\n}}\n",
            tfname(&s.service), s.service, s.engine
        ));
    }
    for s in &p.secrets {
        o.push(format!(
            "resource \"aws_secretsmanager_secret\" \"{}_{}\" {{\n  name = \"{}\"\n}}\n",
            tfname(&s.service),
            tfname(&s.key),
            s.name
        ));
    }
    o.join("\n")
}

/// Knative Eventing: el target realmente portable — el mismo YAML corre en
/// cualquier Kubernetes, con el broker que tenga detras (Kafka, RabbitMQ, GCP).
fn k8s(p: &Plan) -> String {
    let mut o = vec![
        "# generado por axon — no editar".to_string(),
        "apiVersion: eventing.knative.dev/v1\nkind: Broker\nmetadata:\n  name: axon\nspec:\n  \
         delivery:\n    deadLetterSink:\n      ref:\n        apiVersion: v1\n        kind: Service\n        name: axon-dlq"
            .to_string(),
    ];
    for s in &p.subs {
        o.push(format!(
            "---\napiVersion: eventing.knative.dev/v1\nkind: Trigger\nmetadata:\n  name: {}\nspec:\n  \
             broker: axon\n  filter:\n    attributes:\n      type: {}\n  delivery:\n    retry: {}\n    \
             backoffPolicy: exponential\n    deadLetterSink:\n      ref:\n        apiVersion: v1\n        \
             kind: Service\n        name: axon-dlq\n  subscriber:\n    ref:\n      apiVersion: v1\n      \
             kind: Service\n      name: {}",
            tfname(&s.name), s.event, s.max_attempts, s.service
        ));
    }
    for s in &p.secrets {
        o.push(format!(
            "---\n# el valor no vive aqui: lo sincroniza External Secrets desde tu vault\napiVersion: \
             external-secrets.io/v1beta1\nkind: ExternalSecret\nmetadata:\n  name: {}\nspec:\n  \
             target:\n    name: {}\n  data:\n    - secretKey: {}\n      remoteRef:\n        key: {}",
            s.name, s.service, s.key, s.name
        ));
    }
    o.join("\n")
}

/// El mismo plan, en tu laptop. Ese es el punto: local y produccion salen de
/// la misma declaracion, asi que no pueden divergir. Postgres por servicio
/// (database-per-service tambien en local) y NATS JetStream como broker.
fn local(p: &Plan) -> String {
    let mut o = String::from(
        "# generado por axon — no editar.  docker compose -f axon.local.yml up -d
services:
  broker:
    image: nats:2-alpine
    command: [\"-js\", \"-m\", \"8222\"]
    ports: [\"4222:4222\", \"8222:8222\"]
    healthcheck:
      test: [\"CMD\", \"wget\", \"-qO-\", \"http://localhost:8222/healthz\"]
      interval: 2s
",
    );
    for (i, s) in p.stores.iter().enumerate() {
        let svc = &s.service;
        let port = 5432 + i;
        o.push_str(&format!(
            "  db-{svc}:
    image: postgres:16-alpine
    environment: {{ POSTGRES_DB: {svc}, POSTGRES_PASSWORD: local }}
    ports: [\"{port}:5432\"]
    healthcheck:
      test: [\"CMD-SHELL\", \"pg_isready -U postgres\"]
      interval: 2s
  migrate-{svc}:
    image: flyway/flyway:10-alpine
    depends_on: {{ db-{svc}: {{ condition: service_healthy }} }}
    volumes: [\"./sql/{svc}:/flyway/sql:ro\"]
    command: >
      -url=jdbc:postgresql://db-{svc}:5432/{svc}
      -user=postgres -password=local -connectRetries=10 migrate
"
        ));
    }
    o.push_str("\n# streams JetStream a crear al arrancar:\n");
    for t in &p.topics {
        o.push_str(&format!(
            "#   nats stream add {} --subjects {}\n",
            t.name, t.name
        ));
    }
    o.push_str("# los secretos en local se leen de .env.local, nunca del vault:\n");
    for s in &p.secrets {
        o.push_str(&format!("#   {}\n", s.key));
    }
    o
}
