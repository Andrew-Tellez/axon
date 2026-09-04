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
    /// Standby con failover. No se lee de el, asi que no rompe consistencia.
    pub ha: bool,
    pub backup_retention_days: u32,
    pub pitr: bool,
    /// Replicas de las que SI se lee, con el retraso que eso implica.
    pub read_replicas: u32,
    pub pool_size: Option<u32>,
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
    /// Para los atributos de recurso de OpenTelemetry: una traza sin dueño ni
    /// criticidad no sirve a las 3am.
    pub owner: String,
    pub tier: String,
    pub version: String,
}

/// Una ruta del edge. El gateway no es una fuente de verdad nueva: sale de
/// los metodos que cada servicio declara con `http`.
#[derive(Debug, Serialize)]
pub struct Route {
    pub method: String,
    pub path: String,
    pub service: String,
    pub port: u16,
    pub public: bool,
    pub rate_limit: Option<u32>,
    pub timeout_ms: u32,
}

/// Almacenamiento de objetos, y su CDN si es publico.
#[derive(Debug, Serialize)]
pub struct Store2 {
    pub service: String,
    pub name: String,
    /// Nombre global: los buckets comparten espacio de nombres en todo el mundo.
    pub bucket: String,
    pub public: bool,
    pub retention_days: Option<u32>,
    pub cache_ttl: u32,
}

/// Plan neutral. Sin una sola palabra de ningun proveedor.
#[derive(Debug, Serialize)]
pub struct Plan {
    pub buckets: Vec<Store2>,
    pub routes: Vec<Route>,
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

    let mut routes = Vec::new();
    let mut buckets = Vec::new();
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
                ha: m.infra.ha.unwrap_or(false),
                backup_retention_days: m.infra.backup_retention_days.unwrap_or(0),
                pitr: m.infra.pitr.unwrap_or(false),
                read_replicas: m.infra.read_replicas.unwrap_or(0),
                pool_size: m.infra.pool_size,
            });
        }
        for (_, me) in m.methods.iter() {
            let (Some(verbo), Some(ruta)) = (me.verb(), me.path()) else {
                continue;
            };
            routes.push(Route {
                method: verbo.to_string(),
                path: ruta.to_string(),
                service: svc.clone(),
                port: m.infra.port.unwrap_or(8080),
                public: me.auth.as_deref() == Some("public"),
                rate_limit: me.rate_limit,
                timeout_ms: me.timeout_ms.unwrap_or(10_000),
            });
        }
        for (nombre, b) in &m.infra.buckets {
            buckets.push(Store2 {
                service: svc.clone(),
                name: nombre.clone(),
                // Plantilla neutral: `{project}` lo sustituye cada target con su
                // propia sintaxis. El plan no lleva interpolacion de nadie.
                bucket: format!("{{project}}-{svc}-{nombre}"),
                public: b.public,
                retention_days: b.retention_days,
                cache_ttl: b.cache_ttl.unwrap_or(3600),
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
            owner: m.owner.clone().unwrap_or_default(),
            tier: m.tier.clone().unwrap_or_default(),
            version: m.version.clone().unwrap_or_default(),
        });
    }
    routes.sort_by(|a, b| (&a.path, &a.method).cmp(&(&b.path, &b.method)));
    Plan {
        buckets,
        routes,
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
    const PROY: &str = "${var.project}";
    let mut o = vec![HEAD.to_string()];
    for w in &p.workloads {
        o.push(format!(
            "variable \"{}\" {{ type = string }}\n",
            w.image_var
        ));
    }
    if !p.workloads.is_empty() {
        // el backend de trazas no lo elige axon: solo dice donde exportar
        o.push(
            "variable \"otlp_endpoint\" {\n  type        = string\n  \
             description = \"Colector OTLP: Cloud Trace via OpenTelemetry Collector, o el que uses\"\n}\n"
                .to_string(),
        );
    }
    if !p.stores.is_empty() {
        o.push(
            "variable \"db_tier\" {\n  type        = string\n  \
             description = \"Tamano de instancia de Cloud SQL, por ejemplo db-custom-2-7680\"\n}\n"
                .to_string(),
        );
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
        for (k, v) in env_buckets(p, &w.service, PROY)
            .into_iter()
            .chain(env_otel(w, "${var.otlp_endpoint}"))
        {
            env.push_str(&format!(
                "        env {{\n          name  = \"{k}\"\n          value = \"{v}\"\n        }}\n"
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
             location = var.region\n  \
             # [A01] sin ruta publica no hay puerta a internet\n  \
             ingress  = \"{ingress}\"\n  template {{\n    service_account = google_service_account.{s}.email\n    \
             scaling {{\n      min_instance_count = {min}\n      max_instance_count = {max}\n    }}\n    \
             containers {{\n      image = var.{img}\n      ports {{\n        container_port = {port}\n      }}\n\
{env}    }}\n  }}\n}}\n",
            svc = w.service, min = w.min_instances, max = w.max_instances,
            img = w.image_var, port = w.port,
            ingress = if p.routes.iter().any(|r| r.service == w.service && r.public) {
                "INGRESS_TRAFFIC_ALL"
            } else {
                "INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER"
            }
        ));
    }
    if !p.routes.is_empty() {
        let mut reglas = String::new();
        for r in &p.routes {
            let prefijo = match r.path.find('{') {
                Some(i) => r.path[..i].trim_end_matches('/').to_string(),
                None => r.path.clone(),
            };
            reglas.push_str(&format!(
                "    path_rule {{\n      paths   = [\"{prefijo}\", \"{prefijo}/*\"]\n      \
                 service = google_compute_backend_service.{}.id\n    }}\n",
                tfname(&r.service)
            ));
        }
        let mut vistos: Vec<&str> = p.routes.iter().map(|r| r.service.as_str()).collect();
        vistos.sort();
        vistos.dedup();
        for svc in vistos {
            o.push(format!(
                "resource \"google_compute_region_network_endpoint_group\" \"{n}\" {{\n  \
                 name                  = \"{svc}-neg\"\n  region                = var.region\n  \
                 network_endpoint_type = \"SERVERLESS\"\n  \
                 cloud_run {{\n    service = google_cloud_run_v2_service.{n}.name\n  }}\n}}\n\n\
                 resource \"google_compute_backend_service\" \"{n}\" {{\n  \
                 name = \"{svc}-backend\"\n  \
                 backend {{\n    group = google_compute_region_network_endpoint_group.{n}.id\n  }}\n}}\n",
                n = tfname(svc)
            ));
        }
        o.push(format!(
            "resource \"google_compute_url_map\" \"edge\" {{\n  name            = \"axon-edge\"\n  \
             default_service = google_compute_backend_service.{}.id\n\n  \
             path_matcher {{\n    name            = \"axon\"\n    \
             default_service = google_compute_backend_service.{}.id\n{reglas}  }}\n}}\n",
            tfname(&p.routes[0].service), tfname(&p.routes[0].service)
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
            "resource \"google_sql_database_instance\" \"{sv}\" {{\n  \
             name             = \"{svc}\"\n  \
             database_version = \"POSTGRES_16\"\n  \
             region           = var.region\n  \
             # una base por servicio quiere decir una INSTANCIA por servicio: con\n  \
             # todas en la misma, un vecino ruidoso las tira juntas\n  \
             deletion_protection = true\n  \
             settings {{\n    \
               tier              = var.db_tier\n    \
               # REGIONAL es el standby con failover; del standby no se lee\n    \
               availability_type = \"{disp}\"\n{respaldo}  }}\n}}\n",
            svc = s.service,
            disp = if s.ha { "REGIONAL" } else { "ZONAL" },
            respaldo = if s.backup_retention_days > 0 {
                format!(
                    "    backup_configuration {{\n      enabled                        = true\n      \
                     point_in_time_recovery_enabled = {}\n      \
                     backup_retention_settings {{\n        retained_backups = {}\n      }}\n    }}\n",
                    s.pitr, s.backup_retention_days
                )
            } else {
                String::new()
            }
        ));
        o.push(format!(
            "resource \"google_sql_database\" \"{sv}\" {{\n  name     = \"{}\"\n  \
             instance = google_sql_database_instance.{sv}.name\n}}\n",
            s.service
        ));
        for i in 1..=s.read_replicas {
            o.push(format!(
                "resource \"google_sql_database_instance\" \"{sv}_ro_{i}\" {{\n  \
                 name                 = \"{svc}-ro-{i}\"\n  \
                 database_version     = \"POSTGRES_16\"\n  \
                 region               = var.region\n  \
                 master_instance_name = google_sql_database_instance.{sv}.name\n  \
                 deletion_protection  = false\n  \
                 # una replica no lleva respaldo propio: se respalda el primario\n  \
                 replica_configuration {{\n    failover_target = false\n  }}\n  \
                 settings {{\n    tier              = var.db_tier\n    \
                 availability_type = \"ZONAL\"\n  }}\n}}\n",
                svc = s.service
            ));
        }
        o.push(format!(
            "resource \"google_secret_manager_secret\" \"{sv}_database_url\" {{\n  \
             secret_id = \"{}-database-url\"\n  replication {{\n    auto {{}}\n  }}\n}}\n",
            s.service
        ));
        if s.outbox {
            o.push(format!(
                "resource \"google_sql_user\" \"{sv}_relay\" {{\n  \
                 name     = \"{}-outbox-relay\"\n  \
                 instance = google_sql_database_instance.{sv}.name\n}}\n",
                s.service
            ));
        }
    }
    for b in &p.buckets {
        let n = tfname(&format!("{}-{}", b.service, b.name));
        o.push(format!(
            "resource \"google_storage_bucket\" \"{n}\" {{\n  name          = \"{}\"\n  \
             location      = var.region\n  uniform_bucket_level_access = true\n  \
             public_access_prevention    = \"{}\"\n{}}}\n",
            b.bucket.replace("{project}", "${var.project}"),
            if b.public { "inherited" } else { "enforced" },
            b.retention_days
                .map(|d| format!(
                    "  lifecycle_rule {{\n    condition {{\n      age = {d}\n    }}\n    \
                     action {{\n      type = \"Delete\"\n    }}\n  }}\n"
                ))
                .unwrap_or_default()
        ));
        if b.public {
            o.push(format!(
                "resource \"google_storage_bucket_iam_member\" \"{n}_publico\" {{\n  \
                 bucket = google_storage_bucket.{n}.name\n  role   = \"roles/storage.objectViewer\"\n  \
                 member = \"allUsers\"\n}}\n\n\
                 resource \"google_compute_backend_bucket\" \"{n}\" {{\n  \
                 name        = \"{}-cdn\"\n  bucket_name = google_storage_bucket.{n}.name\n  \
                 enable_cdn  = true\n  cdn_policy {{\n    cache_mode  = \"CACHE_ALL_STATIC\"\n    \
                 default_ttl = {}\n  }}\n}}\n",
                b.bucket.replace("{project}", "${var.project}"), b.cache_ttl
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
    const PROY: &str = "${var.project}";
    let mut o = vec![HEAD.to_string()];
    for w in &p.workloads {
        o.push(format!(
            "variable \"{}\" {{ type = string }}\n",
            w.image_var
        ));
    }
    if !p.workloads.is_empty() {
        o.push(
            "variable \"otlp_endpoint\" {\n  type        = string\n  \
             description = \"Colector OTLP: el ADOT Collector hacia X-Ray, o el que uses\"\n}\n"
                .to_string(),
        );
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
             portMappings = [{{ containerPort = {port} }}]\n    environment = [{env}]\n    \
             secrets = [{sec}]\n  }}])\n}}\n",
            svc = w.service,
            img = w.image_var,
            port = w.port,
            sec = secrets.join(", "),
            env = env_buckets(p, &w.service, PROY)
                .into_iter()
                .chain(env_otel(w, "${var.otlp_endpoint}"))
                .map(|(k, v)| format!("{{ name = \"{k}\", value = \"{v}\" }}"))
                .collect::<Vec<_>>()
                .join(", ")
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
    if !p.routes.is_empty() {
        o.push(
            "resource \"aws_apigatewayv2_api\" \"edge\" {\n  name          = \"axon-edge\"\n  \
             protocol_type = \"HTTP\"\n}\n"
                .to_string(),
        );
        for r in &p.routes {
            let n = tfname(&format!("{}-{}", r.method, r.path));
            o.push(format!(
                "resource \"aws_apigatewayv2_integration\" \"{n}\" {{\n  \
                 api_id                 = aws_apigatewayv2_api.edge.id\n  \
                 integration_type       = \"HTTP_PROXY\"\n  \
                 integration_method     = \"{m}\"\n  \
                 integration_uri        = \"http://{svc}.internal:{port}{path}\"\n  \
                 timeout_milliseconds   = {t}\n}}\n",
                m = r.method,
                svc = r.service,
                port = r.port,
                path = r.path,
                t = r.timeout_ms
            ));
            o.push(format!(
                "resource \"aws_apigatewayv2_route\" \"{n}\" {{\n  \
                 api_id             = aws_apigatewayv2_api.edge.id\n  \
                 route_key          = \"{m} {path}\"\n  \
                 target             = \"integrations/${{aws_apigatewayv2_integration.{n}.id}}\"\n  \
                 authorization_type = \"{auth}\"\n}}\n",
                m = r.method,
                path = r.path,
                auth = if r.public { "NONE" } else { "JWT" }
            ));
        }
    }
    for s in &p.stores {
        let sv = tfname(&s.service);
        o.push(format!(
            "resource \"aws_db_instance\" \"{sv}\" {{\n  \
             identifier        = \"{svc}\"\n  \
             engine            = \"{eng}\"\n  \
             instance_class    = var.db_instance_class\n  \
             allocated_storage = 20\n  \
             # multi_az es el standby con failover; no se lee de el\n  \
             multi_az                = {ha}\n  \
             # en RDS, retencion > 0 ya habilita recuperacion a un punto en el\n  \
             # tiempo: no hay un atributo `pitr` aparte\n  \
             backup_retention_period = {ret}\n  \
             deletion_protection     = true\n  \
             skip_final_snapshot     = false\n}}\n",
            svc = s.service,
            eng = s.engine,
            ha = s.ha,
            ret = s.backup_retention_days
        ));
        for i in 1..=s.read_replicas {
            o.push(format!(
                "resource \"aws_db_instance\" \"{sv}_ro_{i}\" {{\n  \
                 identifier          = \"{svc}-ro-{i}\"\n  \
                 replicate_source_db = aws_db_instance.{sv}.identifier\n  \
                 instance_class      = var.db_instance_class\n  \
                 # una replica no lleva respaldo propio\n  \
                 backup_retention_period = 0\n  \
                 skip_final_snapshot     = true\n}}\n",
                svc = s.service
            ));
        }
        o.push(format!(
            "resource \"aws_secretsmanager_secret\" \"{sv}_database_url\" {{\n  name = \"{}-database-url\"\n}}\n",
            s.service
        ));
    }
    for b in &p.buckets {
        let n = tfname(&format!("{}-{}", b.service, b.name));
        o.push(format!(
            "resource \"aws_s3_bucket\" \"{n}\" {{\n  bucket = \"{}\"\n}}\n",
            b.bucket.replace("{project}", "${var.project}")
        ));
        o.push(format!(
            "resource \"aws_s3_bucket_public_access_block\" \"{n}\" {{\n  \
             bucket                  = aws_s3_bucket.{n}.id\n  \
             block_public_acls       = true\n  block_public_policy     = {}\n  \
             ignore_public_acls      = true\n  restrict_public_buckets = {}\n}}\n",
            !b.public, !b.public
        ));
        if let Some(d) = b.retention_days {
            o.push(format!(
                "resource \"aws_s3_bucket_lifecycle_configuration\" \"{n}\" {{\n  \
                 bucket = aws_s3_bucket.{n}.id\n  rule {{\n    id     = \"retencion\"\n    \
                 status = \"Enabled\"\n    filter {{}}\n    expiration {{\n      days = {d}\n    }}\n  }}\n}}\n"
            ));
        }
        if b.public {
            o.push(format!(
                "resource \"aws_cloudfront_distribution\" \"{n}\" {{\n  enabled = true\n  \
                 origin {{\n    domain_name = aws_s3_bucket.{n}.bucket_regional_domain_name\n    \
                 origin_id   = \"{n}\"\n  }}\n  \
                 default_cache_behavior {{\n    target_origin_id       = \"{n}\"\n    \
                 viewer_protocol_policy = \"redirect-to-https\"\n    \
                 allowed_methods        = [\"GET\", \"HEAD\"]\n    \
                 cached_methods         = [\"GET\", \"HEAD\"]\n    \
                 default_ttl            = {}\n  }}\n  \
                 restrictions {{\n    geo_restriction {{\n      restriction_type = \"none\"\n    }}\n  }}\n  \
                 viewer_certificate {{\n    cloudfront_default_certificate = true\n  }}\n}}\n",
                b.cache_ttl
            ));
        }
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
    const PROY: &str = "${PROJECT}";
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
        for (k, v) in env_buckets(p, &w.service, PROY)
            .into_iter()
            .chain(env_otel(w, "${OTLP_ENDPOINT}"))
        {
            env.push_str(&format!(
                "            - name: {k}\n              value: \"{v}\"\n"
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
      # [A05] endurecido por generacion, no por acordarse
      securityContext:
        runAsNonRoot: true
        runAsUser: 10001
        fsGroup: 10001
        seccompProfile: {{ type: RuntimeDefault }}
      automountServiceAccountToken: false
      containers:
        - name: {svc}
          image: IMAGE_{up}
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities: {{ drop: [\"ALL\"] }}
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
    if !p.routes.is_empty() {
        o.push(
            "---
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: axon-edge
spec:
  gatewayClassName: ${GATEWAY_CLASS}
  listeners:
    - name: https
      protocol: HTTPS
      port: 443"
                .to_string(),
        );
        for r in &p.routes {
            // Gateway API no entiende `{param}`: una ruta con parametro se
            // enruta por prefijo hasta el ultimo segmento fijo.
            let (tipo, valor) = match r.path.find('{') {
                Some(i) => ("PathPrefix", r.path[..i].trim_end_matches('/').to_string()),
                None => ("Exact", r.path.clone()),
            };
            o.push(format!(
                "---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: {n}
  annotations:
    axon.dev/auth: {auth}{rl}
spec:
  parentRefs:
    - name: axon-edge
  rules:
    - matches:
        - path: {{ type: {tipo}, value: {valor} }}
          method: {metodo}
      timeouts: {{ request: {t}s }}
      backendRefs:
        - name: {svc}
          port: 80",
                n = tfname(&format!("{}-{}", r.method, r.path)),
                auth = if r.public { "public" } else { "required" },
                rl = r
                    .rate_limit
                    .map(|v| format!("\n    axon.dev/rate-limit: \"{v}\""))
                    .unwrap_or_default(),
                metodo = r.method,
                t = r.timeout_ms / 1000,
                svc = r.service,
            ));
        }
    }
    for w in &p.workloads {
        // [A01/A05] denegar por defecto: al pod solo entra el edge, y solo si
        // el servicio expone rutas. Un servicio sin ruta no es alcanzable
        // desde fuera, aunque alguien se equivoque en el gateway.
        let desde_edge = p.routes.iter().any(|r| r.service == w.service);
        let reglas = if desde_edge {
            "\n    - from:\n        - namespaceSelector:\n            matchLabels: { axon.dev/edge: \"true\" }"
        } else {
            " []   # nadie: este servicio solo reacciona a eventos"
        };
        o.push(format!(
            "---\napiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata:\n  name: {svc}\nspec:\n  \
             podSelector:\n    matchLabels: {{ app: {svc} }}\n  policyTypes: [Ingress]\n  ingress:{reglas}",
            svc = w.service
        ));
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
    const PROY: &str = "local";
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
    // Jaeger all-in-one acepta OTLP directo, asi que el backend de trazas es
    // un contenedor y no un colector mas un almacen.
    o.push_str(
        "  traza:
    image: jaegertracing/all-in-one:1.76.0
    environment: { COLLECTOR_OTLP_ENABLED: \"true\" }
    ports: [\"${AXON_OTLP_PORT:-4318}:4318\", \"${AXON_TRAZA_UI_PORT:-16686}:16686\"]
    healthcheck:
      test: [\"CMD\", \"wget\", \"-qO-\", \"http://localhost:14269/\"]
      interval: 2s
",
    );
    if !p.buckets.is_empty() {
        o.push_str(
            "  objetos:
    image: minio/minio:latest
    command: [\"server\", \"/data\", \"--console-address\", \":9001\"]
    environment: { MINIO_ROOT_USER: local, MINIO_ROOT_PASSWORD: locallocal }
    ports: [\"${AXON_S3_PORT:-9000}:9000\", \"${AXON_S3_CONSOLE_PORT:-9001}:9001\"]
    healthcheck:
      test: [\"CMD\", \"mc\", \"ready\", \"local\"]
      interval: 2s
",
        );
        o.push_str("  crear-buckets:\n    image: minio/mc:latest\n    depends_on: { objetos: { condition: service_healthy } }\n    entrypoint: >\n      /bin/sh -c \"mc alias set local http://objetos:9000 local locallocal");
        for b in &p.buckets {
            o.push_str(&format!(
                " && mc mb -p local/{}",
                b.bucket.replace("{project}", "local")
            ));
        }
        o.push_str("\"\n");
    }
    if !p.routes.is_empty() {
        o.push_str(
            "  edge:
    image: traefik:v3
    command:
      - --providers.docker=true
      - --entrypoints.web.address=:80
    ports: [\"${AXON_EDGE_PORT:-8000}:80\"]
    volumes: [\"/var/run/docker.sock:/var/run/docker.sock:ro\"]
",
        );
    }
    for (i, w) in p.workloads.iter().enumerate() {
        let svc = &w.service;
        let mut deps = vec![
            "broker: { condition: service_healthy }".to_string(),
            "traza: { condition: service_healthy }".to_string(),
        ];
        // No arrancar la app antes de que existan sus buckets. Y sin esto,
        // `up --wait` cuenta el job de creacion como un contenedor caido.
        if !env_buckets(p, &w.service, PROY).is_empty() {
            deps.push("crear-buckets: { condition: service_completed_successfully }".into());
        }
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
        let mut secrets: String = w
            .secrets
            .iter()
            .map(|s| format!("      # {s}: viene de .env.local\n"))
            .collect();
        if !env_buckets(p, &w.service, PROY).is_empty() {
            secrets.push_str("      AWS_ENDPOINT_URL: http://objetos:9000\n");
        }
        for (k, v) in env_buckets(p, &w.service, PROY) {
            secrets.push_str(&format!("      {k}: {}\n", v));
        }
        for (k, v) in env_otel_con(w, "http://traza:4318", true) {
            secrets.push_str(&format!("      {k}: \"{v}\"\n"));
        }
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
{labels}",
            deps = deps.join(", "),
            host = 8080 + i,
            port = w.port,
            v = tfname(&w.service),
            labels = etiquetas_edge(p, &w.service)
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

/// Las mismas rutas del plan, como reglas de Traefik. Local no es un
/// subsistema aparte: es otro render del mismo edge.
fn etiquetas_edge(p: &Plan, svc: &str) -> String {
    let mias: Vec<&Route> = p.routes.iter().filter(|r| r.service == svc).collect();
    if mias.is_empty() {
        return String::new();
    }
    let reglas: Vec<String> = mias
        .iter()
        .map(|r| {
            let prefijo = match r.path.find('{') {
                Some(i) => r.path[..i].trim_end_matches('/').to_string(),
                None => r.path.clone(),
            };
            format!("PathPrefix(`{prefijo}`)")
        })
        .collect();
    let mut u: Vec<String> = reglas;
    u.sort();
    u.dedup();
    format!(
        "    labels:\n      \
         - traefik.enable=true\n      \
         - traefik.http.routers.{svc}.rule={}\n      \
         - traefik.http.services.{svc}.loadbalancer.server.port={}\n",
        u.join(" || "),
        mias[0].port
    )
}

/// El nombre del bucket es distinto en cada entorno, asi que la app lo lee de
/// una variable, no lo construye. Mismo nombre de variable en los cuatro targets.
fn env_buckets(p: &Plan, svc: &str, proyecto: &str) -> Vec<(String, String)> {
    p.buckets
        .iter()
        .filter(|b| b.service == svc)
        .map(|b| {
            (
                format!("BUCKET_{}", b.name.to_uppercase()),
                b.bucket.replace("{project}", proyecto),
            )
        })
        .collect()
}

/// Variables estandar de OpenTelemetry. axon no trae un SDK ni inventa un
/// formato: el envelope ya propaga `traceparent`, que es el contexto W3C que
/// usa OTel, asi que basta con levantar el backend y decirle al SDK del equipo
/// donde exportar. Los atributos de recurso salen del manifiesto.
fn env_otel(w: &Workload, endpoint: &str) -> Vec<(String, String)> {
    env_otel_con(w, endpoint, false)
}

/// `todo` fuerza el muestreo completo: el muestreo existe para controlar
/// volumen en produccion, no para esconderte el 90% de las trazas mientras
/// depuras en tu laptop.
fn env_otel_con(w: &Workload, endpoint: &str, todo: bool) -> Vec<(String, String)> {
    let mut atributos = vec![
        format!("service.name={}", w.service),
        format!("axon.owner={}", w.owner),
        format!("axon.tier={}", w.tier),
    ];
    if !w.version.is_empty() {
        atributos.push(format!("service.version={}", w.version));
    }
    let mut v = vec![
        ("OTEL_SERVICE_NAME".into(), w.service.clone()),
        ("OTEL_EXPORTER_OTLP_ENDPOINT".into(), endpoint.to_string()),
        ("OTEL_EXPORTER_OTLP_PROTOCOL".into(), "http/protobuf".into()),
        ("OTEL_RESOURCE_ATTRIBUTES".into(), atributos.join(",")),
    ];
    // Un servicio tier 0 se muestrea entero: cuando se cae, la traza que falta
    // es justo la que hacia falta.
    if todo || w.tier == "0" {
        v.push(("OTEL_TRACES_SAMPLER".into(), "parentbased_always_on".into()));
    } else {
        v.push((
            "OTEL_TRACES_SAMPLER".into(),
            "parentbased_traceidratio".into(),
        ));
        v.push(("OTEL_TRACES_SAMPLER_ARG".into(), "0.1".into()));
    }
    v
}
