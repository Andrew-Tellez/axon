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
    /// Consumidor: tambien es el destino de entrega.
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
    pub max_instances: u32,
    pub port: u16,
    /// La imagen no se declara: es distinta en cada deploy, asi que sale
    /// como variable del IaC.
    pub image_var: String,
    pub db: bool,
    pub secrets: Vec<String>,
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
            max_instances: m.infra.max_instances.unwrap_or(10),
            port: m.infra.port.unwrap_or(8080),
            image_var: format!("{}_image", tfname(svc)),
            db: m.infra.state.is_some(),
            secrets: m.infra.secrets.clone(),
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
    for w in &p.workloads {
        o.push(format!(
            "variable \"{}\" {{ type = string }}\n",
            w.image_var
        ));
    }
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
    for w in &p.workloads {
        let s = tfname(&w.service);
        o.push(format!(
            "resource \"google_service_account\" \"{s}\" {{\n  account_id = \"{}\"\n}}\n",
            w.service
        ));
        let mut env = String::new();
        if w.db {
            env.push_str(&format!(
                "        env {{\n          name = \"DATABASE_URL\"\n          value_source {{\n            \
                 secret_key_ref {{\n              secret  = google_secret_manager_secret.{s}_database_url.secret_id\n              version = \"latest\"\n            }}\n          }}\n        }}\n"
            ));
        }
        for sec in &w.secrets {
            env.push_str(&format!(
                "        env {{\n          name = \"{sec}\"\n          value_source {{\n            \
                 secret_key_ref {{\n              secret  = google_secret_manager_secret.{s}_{}.secret_id\n              version = \"latest\"\n            }}\n          }}\n        }}\n",
                tfname(sec)
            ));
        }
        o.push(format!(
            "resource \"google_cloud_run_v2_service\" \"{s}\" {{\n  name     = \"{svc}\"\n  \
             location = var.region\n  template {{\n    service_account = google_service_account.{s}.email\n    \
             scaling {{\n      min_instance_count = {min}\n      max_instance_count = {max}\n    }}\n    \
             containers {{\n      image = var.{img}\n      ports {{\n        container_port = {port}\n      }}\n\
{env}    }}\n  }}\n}}\n",
            svc = w.service, min = w.min_instances, max = w.max_instances,
            img = w.image_var, port = w.port
        ));
    }
    for s in &p.subs {
        let (n, sv) = (tfname(&s.event), tfname(&s.service));
        // push: la suscripcion entrega al workload, no al vacio
        o.push(format!(
            "resource \"google_pubsub_subscription\" \"{sv}_{n}\" {{\n  name  = \"{}\"\n  \
             topic = google_pubsub_topic.{n}.name\n  push_config {{\n    \
             push_endpoint = google_cloud_run_v2_service.{sv}.uri\n    \
             oidc_token {{\n      service_account_email = google_service_account.{sv}.email\n    }}\n  }}\n  \
             dead_letter_policy {{\n    dead_letter_topic     = google_pubsub_topic.{n}_dlq.id\n    \
             max_delivery_attempts = {}\n  }}\n}}\n",
            s.name, s.max_attempts
        ));
    }
    for s in &p.stores {
        let sv = tfname(&s.service);
        o.push(format!(
            "resource \"google_sql_database\" \"{sv}\" {{\n  name     = \"{}\"\n  instance = var.sql_instance\n}}\n",
            s.service
        ));
        o.push(format!(
            "resource \"google_secret_manager_secret\" \"{sv}_database_url\" {{\n  \
             secret_id = \"{}-database-url\"\n  replication {{\n    auto {{}}\n  }}\n}}\n",
            s.service
        ));
        if s.outbox {
            o.push(format!(
                "resource \"google_sql_user\" \"{sv}_relay\" {{\n  name     = \"{}-outbox-relay\"\n  instance = var.sql_instance\n}}\n",
                s.service
            ));
        }
    }
    for s in &p.secrets {
        o.push(format!(
            "resource \"google_secret_manager_secret\" \"{}_{}\" {{\n  secret_id = \"{}\"\n  replication {{\n    auto {{}}\n  }}\n}}\n",
            tfname(&s.service), tfname(&s.key), s.name
        ));
    }
    o.join("\n")
}

fn aws(p: &Plan) -> String {
    let mut o = vec![HEAD.to_string()];
    for w in &p.workloads {
        o.push(format!(
            "variable \"{}\" {{ type = string }}\n",
            w.image_var
        ));
    }
    for t in &p.topics {
        o.push(format!(
            "resource \"aws_sns_topic\" \"{}\" {{\n  name = \"{}\"\n}}\n",
            tfname(&t.event),
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
    for w in &p.workloads {
        let s = tfname(&w.service);
        let mut secrets: Vec<String> = Vec::new();
        if w.db {
            secrets.push(format!(
                "{{ name = \"DATABASE_URL\", valueFrom = aws_secretsmanager_secret.{s}_database_url.arn }}"
            ));
        }
        for sec in &w.secrets {
            secrets.push(format!(
                "{{ name = \"{sec}\", valueFrom = aws_secretsmanager_secret.{s}_{}.arn }}",
                tfname(sec)
            ));
        }
        // la cola que consume este workload, para que el autoscaler la mire
        let queues: Vec<String> = p
            .subs
            .iter()
            .filter(|x| x.service == w.service)
            .map(|x| format!("aws_sqs_queue.{}.arn", tfname(&x.name)))
            .collect();
        o.push(format!(
            "resource \"aws_ecs_task_definition\" \"{s}\" {{\n  family                   = \"{svc}\"\n  \
             requires_compatibilities = [\"FARGATE\"]\n  network_mode             = \"awsvpc\"\n  \
             cpu = \"512\"\n  memory = \"1024\"\n  execution_role_arn = var.ecs_execution_role_arn\n  \
             container_definitions = jsonencode([{{\n    name  = \"{svc}\"\n    image = var.{img}\n    \
             portMappings = [{{ containerPort = {port} }}]\n    secrets = [{sec}]\n  }}])\n}}\n",
            svc = w.service, img = w.image_var, port = w.port, sec = secrets.join(", ")
        ));
        o.push(format!(
            "resource \"aws_ecs_service\" \"{s}\" {{\n  name            = \"{svc}\"\n  \
             cluster         = var.ecs_cluster\n  task_definition = aws_ecs_task_definition.{s}.arn\n  \
             desired_count   = {min}\n  launch_type     = \"FARGATE\"\n  \
             network_configuration {{\n    subnets = var.subnets\n  }}\n}}\n",
            svc = w.service, min = w.min_instances.max(1)
        ));
        if !queues.is_empty() {
            o.push(format!(
                "# {svc} escala con la profundidad de sus colas: {}\n\
                 resource \"aws_appautoscaling_target\" \"{s}\" {{\n  \
                 service_namespace  = \"ecs\"\n  resource_id        = \"service/${{var.ecs_cluster}}/{svc}\"\n  \
                 scalable_dimension = \"ecs:service:DesiredCount\"\n  min_capacity = {min}\n  max_capacity = {max}\n}}\n",
                queues.join(", "), svc = w.service, min = w.min_instances.max(1), max = w.max_instances
            ));
        }
    }
    for s in &p.stores {
        let sv = tfname(&s.service);
        o.push(format!(
            "resource \"aws_db_instance\" \"{sv}\" {{\n  identifier        = \"{}\"\n  engine            = \"{}\"\n  \
             instance_class    = var.db_instance_class\n  allocated_storage = 20\n}}\n",
            s.service, s.engine
        ));
        o.push(format!(
            "resource \"aws_secretsmanager_secret\" \"{sv}_database_url\" {{\n  name = \"{}-database-url\"\n}}\n",
            s.service
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
    for w in &p.workloads {
        let svc = &w.service;
        let mut env = String::new();
        if w.db {
            env.push_str(&format!(
                "            - name: DATABASE_URL\n              valueFrom:\n                \
                 secretKeyRef: {{ name: {svc}, key: DATABASE_URL }}\n"
            ));
        }
        for sec in &w.secrets {
            env.push_str(&format!(
                "            - name: {sec}\n              valueFrom:\n                \
                 secretKeyRef: {{ name: {svc}, key: {sec} }}\n"
            ));
        }
        o.push(format!(
            "---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {svc}
  labels: {{ app: {svc} }}
spec:
  replicas: {min}
  selector:
    matchLabels: {{ app: {svc} }}
  template:
    metadata:
      labels: {{ app: {svc} }}
    spec:
      containers:
        - name: {svc}
          image: IMAGE_{up}
          ports:
            - containerPort: {port}
          readinessProbe:
            httpGet: {{ path: /healthz, port: {port} }}
          env:
{env}---
apiVersion: v1
kind: Service
metadata:
  name: {svc}
spec:
  selector: {{ app: {svc} }}
  ports:
    - port: 80
      targetPort: {port}",
            min = w.min_instances.max(1),
            port = w.port,
            up = svc.to_uppercase()
        ));
        if w.max_instances > w.min_instances.max(1) {
            o.push(format!(
                "---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: {svc}
spec:
  scaleTargetRef: {{ apiVersion: apps/v1, kind: Deployment, name: {svc} }}
  minReplicas: {min}
  maxReplicas: {max}
  metrics:
    - type: Resource
      resource: {{ name: cpu, target: {{ type: Utilization, averageUtilization: 70 }} }}",
                min = w.min_instances.max(1),
                max = w.max_instances
            ));
        }
    }
    for s in &p.subs {
        o.push(format!(
            "---
apiVersion: eventing.knative.dev/v1
kind: Trigger
metadata:
  name: {}
spec:
  broker: axon
  filter:
    attributes: {{ type: {} }}
  delivery:
    retry: {}
    backoffPolicy: exponential
    deadLetterSink:
      ref: {{ apiVersion: v1, kind: Service, name: axon-dlq }}
  subscriber:
    ref: {{ apiVersion: v1, kind: Service, name: {} }}",
            tfname(&s.name),
            s.event,
            s.max_attempts,
            s.service
        ));
    }
    for s in &p.secrets {
        o.push(format!(
            "---
# el valor no vive aqui: lo sincroniza External Secrets desde tu vault
apiVersion: external-secrets.io/v1beta1
kind: ExternalSecret
metadata:
  name: {}
spec:
  target: {{ name: {} }}
  data:
    - secretKey: {}
      remoteRef: {{ key: {} }}",
            s.name, s.service, s.key, s.name
        ));
    }
    o.join("\n")
}

/// El mismo plan, en tu laptop. Ese es el punto: local y produccion salen de
/// la misma declaracion, asi que no pueden divergir.
fn local(p: &Plan) -> String {
    let mut o = String::from(
        "# generado por axon — no editar.  docker compose -f axon.local.yml up -d --wait
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
        let port = 15432 + i;
        let v = tfname(&s.service);
        o.push_str(&format!(
            "  db-{svc}:
    image: postgres:16-alpine
    environment: {{ POSTGRES_DB: {svc}, POSTGRES_PASSWORD: local }}
    ports: [\"${{AXON_DB_PORT_{v}:-{port}}}:5432\"]
    healthcheck:
      test: [\"CMD-SHELL\", \"pg_isready -U postgres\"]
      interval: 2s
  migrate-{svc}:
    image: flyway/flyway:10-alpine
    depends_on: {{ db-{svc}: {{ condition: service_healthy }} }}
    volumes: [\"./sql/{svc}:/flyway/sql:ro\"]
    command: >
      -url=jdbc:postgresql://db-{svc}:5432/{svc}
      -user=postgres -password=local -connectRetries=10
      -sqlMigrationPrefix= -sqlMigrationSeparator=_
      -validateMigrationNaming=true
      migrate
"
        ));
    }
    // los servicios tuyos, no solo sus dependencias
    for (i, w) in p.workloads.iter().enumerate() {
        let svc = &w.service;
        let mut deps = vec!["broker: { condition: service_healthy }".to_string()];
        if w.db {
            deps.push(format!("db-{svc}: {{ condition: service_healthy }}"));
            deps.push(format!(
                "migrate-{svc}: {{ condition: service_completed_successfully }}"
            ));
        }
        let db_env = if w.db {
            format!("      DATABASE_URL: postgres://postgres:local@db-{svc}:5432/{svc}\n")
        } else {
            String::new()
        };
        let secrets: String = w
            .secrets
            .iter()
            .map(|s| format!("      # {s}: viene de .env.local\n"))
            .collect();
        o.push_str(&format!(
            "  {svc}:
    build:
      context: .
      dockerfile: services/{svc}/Dockerfile
    depends_on: {{ {deps} }}
    ports: [\"${{AXON_PORT_{v}:-{host}}}:{port}\"]
    env_file: [.env.local]
    environment:
      AXON_BROKER_URL: nats://broker:4222
      AXON_TRACE_LOG: /out/local.ndjson
{db_env}{secrets}    volumes: [\"./.axon:/out\"]
",
            deps = deps.join(", "),
            host = 8080 + i,
            port = w.port,
            v = tfname(&w.service)
        ));
    }
    o.push_str("\n# streams JetStream a crear al arrancar:\n");
    for t in &p.topics {
        o.push_str(&format!(
            "#   nats stream add {} --subjects {}\n",
            t.name, t.name
        ));
    }
    o.push_str(
        "# el log de envelopes cae en ./.axon/local.ndjson -> `axon trace .axon/local.ndjson`\n",
    );
    o
}
