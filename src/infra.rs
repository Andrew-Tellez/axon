//! IaC agnostica: el manifiesto produce un PLAN neutral, y cada target lo
//! renderiza. Un proveedor sin target propio se resuelve con `axon plan`,
//! que emite el JSON del plan para que lo rendericen ustedes.
use crate::manifest::*;
use serde::Serialize;
use indexmap::IndexMap;
use std::collections::BTreeSet;

#[derive(Debug, Serialize)]
pub struct Topic {
    pub event: String,
    pub name: String,
    pub dlq: String,
    /// El evento se exporta a la bodega: lleva su propia suscripcion de
    /// escritura directa, aparte de las de los consumidores.
    pub analytics: bool,
    /// Nombre de la tabla destino.
    pub table: String,
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
    /// El tope del motor. `verify` hace la aritmetica contra este numero, asi
    /// que el numero tiene que APLICARSE: una regla que compara contra un tope
    /// que nadie fija esta comparando contra el default del motor, que suele
    /// ser mucho mas bajo.
    pub max_connections: Option<u32>,
    /// El servicio tiene politicas de acceso que aplicar despues de migrar:
    /// `axon rls`. Van aparte porque una politica no es un cambio de esquema.
    pub policies: bool,
    /// Nodos detras del pooler. `None` es sin pooler: el servicio habla
    /// directo con su motor. `Some(n)` son n motores y un pgdog delante, y el
    /// renderer que no sepa repartir tiene que fallar, no emitir uno solo.
    pub shards: Option<u32>,
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

/// Un disparo periodico. El unico que existe hoy sale de `[saga.*]`: el
/// coordinador sabe retomar y hace falta algo que lo llame.
///
/// Va por HTTP contra el propio servicio y no como un comando aparte, porque un
/// comando obliga a un entrypoint distinto en cada lenguaje y esto tiene que
/// funcionar igual en el generador de Go que en el de TypeScript. Una ruta es
/// el unico contrato que todos comparten.
#[derive(Debug, Serialize)]
pub struct Cron {
    pub service: String,
    /// `saga.checkout`, para nombrar el recurso sin ambiguedad.
    pub name: String,
    pub path: String,
    pub port: u16,
    /// Cada cuanto. Sale del presupuesto declarado de la saga: nada se vuelve
    /// elegible antes, asi que disparar mas seguido es trabajo sin resultado.
    pub every_ms: u32,
}

impl Cron {
    /// El intervalo en minutos, redondeado hacia arriba y con piso en 1: los
    /// programadores de los tres proveedores hablan en cron de minutos, y un
    /// intervalo que redondea a 0 se convierte en "cada minuto" sin avisar.
    pub fn minutos(&self) -> u32 {
        self.every_ms.div_ceil(60_000).max(1)
    }

    /// `rate(...)` de EventBridge: con 1 la unidad va en SINGULAR. `rate(1
    /// minutes)` no es un aviso, es un error de validacion.
    pub fn rate(&self) -> String {
        match self.minutos() {
            1 => "rate(1 minute)".to_string(),
            n => format!("rate({n} minutes)"),
        }
    }
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
    /// Hay flags declarados: el target local levanta flagd con su config.
    pub flags: bool,
    /// A que bodega exportan los servicios que exportan. `None` si ninguno lo
    /// hace. Una sola por plataforma: los eventos de un flujo tienen que caer
    /// en el mismo lugar o el embudo no se puede armar.
    pub warehouse: Option<String>,
    pub buckets: Vec<Store2>,
    pub routes: Vec<Route>,
    pub topics: Vec<Topic>,
    pub subs: Vec<Sub>,
    pub stores: Vec<Store>,
    pub crons: Vec<Cron>,
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
            analytics: ms
                .iter()
                .any(|m| m.analytics.export && !m.external && m.emits.contains_key(ev.as_str())),
            table: tfname(ev),
        })
        .collect();

    let mut routes = Vec::new();
    let mut buckets = Vec::new();
    let mut subs = Vec::new();
    let mut stores = Vec::new();
    let mut crons = Vec::new();
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
        for (nombre, ag) in &m.aggregate {
            if ag.snapshot_every == 0 {
                continue;
            }
            crons.push(Cron {
                service: svc.clone(),
                name: format!("fotos.{nombre}"),
                path: format!("/internal/aggregate/{nombre}/limpiar"),
                port: m.infra.port.unwrap_or(8080),
                // Cada hora, y este numero NO sale del manifiesto porque no hay
                // nada ahi de donde derivarlo: la cadencia de fotos se mide en
                // eventos, no en tiempo. Lo que importa es que corra alguna vez;
                // atrasarse solo cuesta espacio.
                every_ms: 3_600_000,
            });
        }
        for (nombre, sg) in &m.saga {
            crons.push(Cron {
                service: svc.clone(),
                name: format!("saga.{nombre}"),
                path: format!("/internal/saga/{nombre}/barrer"),
                port: m.infra.port.unwrap_or(8080),
                every_ms: sg.timeout_ms.unwrap_or(60_000),
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
                max_connections: m.infra.max_connections,
                policies: m.infra.tenant_column.is_some() || !m.pii.is_empty(),
                shards: m.pooler.activo().then_some(m.pooler.shards.max(1)),
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
        flags: ms.iter().any(|m| !m.flags.is_empty()),
        warehouse: ms
            .iter()
            .find(|m| m.analytics.export && !m.external)
            .map(|m| m.analytics.warehouse.clone()),
        buckets,
        routes,
        topics,
        subs,
        stores,
        crons,
        secrets,
        workloads,
    }
}

pub const NATIVE: [&str; 5] = ["local", "gcp", "aws", "k8s", "plan"];

pub fn render(p: &Plan, target: &str) -> Result<String, String> {
    // Solo `local` sabe levantar los nodos del sharder. Emitir UNA instancia
    // donde el manifiesto declara cuatro seria el peor resultado posible: la
    // infraestructura se aplica sin error y el reparto no existe.
    if matches!(target, "gcp" | "aws" | "k8s") {
        if let Some(s) = p.stores.iter().find(|s| s.shards.unwrap_or(1) > 1) {
            return Err(format!(
                "{}: `[pooler] shards = {}` todavia no se renderiza en `{target}`. Hoy el reparto \
                 solo se levanta en `--target local`; emitir una sola instancia aca aplicaria \
                 sin error y dejaria el reparto sin existir. `--target plan` da el plan con \
                 los nodos para renderizarlo con tu plantilla.",
                s.service,
                s.shards.unwrap_or(1)
            ));
        }
    }
    // El esquema de la bodega se genera para tres dialectos, pero el camino que
    // lleva los eventos hasta ahi no existe en todas las combinaciones. Sin
    // este rechazo, `terraform apply` pasa, el esquema se aplica, y las tablas
    // se quedan VACIAS: nadie ve un error y el embudo no dice nada porque no
    // hay filas que decir.
    if let Some(bodega) = &p.warehouse {
        if target != "plan" && !hay_ingesta(target, bodega) {
            return Err(format!(
                "`[analytics] warehouse = \"{bodega}\"` no tiene camino de ingesta en \
                 `{target}`. El esquema se genera igual y las tablas se quedarian vacias \
                 sin un solo error. Combinaciones cableadas: {}. O `export = false` si \
                 este entorno no exporta.",
                INGESTA
                    .iter()
                    .map(|(t, b)| format!("{t}+{b}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
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

/// pgdog solo publica el tag `main`, que se mueve. Actualizarlo es un acto
/// deliberado, igual que el esquema pineado con el que se valida su config.
const PGDOG: &str = "16c85d1c6471de9aefbb2eceae1c48080786d74f9aa8bcadd32fe183c26184a3";

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
    if p.topics.iter().any(|t| t.analytics) {
        o.push(
            "variable \"dataset\" {\n  type        = string\n  \
             description = \"Dataset de BigQuery donde caen los eventos\"\n}\n"
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
    for t in p.topics.iter().filter(|t| t.analytics) {
        let n = tfname(&t.event);
        // Pub/Sub escribe directo en BigQuery: no hay un proceso intermedio
        // que mantener, ni uno mas donde el evento pueda perderse.
        o.push(format!(
            "resource \"google_pubsub_subscription\" \"{n}_bodega\" {{\n  \
             name  = \"bodega--{}\"\n  topic = google_pubsub_topic.{n}.name\n  \
             bigquery_config {{\n    \
               table            = \"${{var.project}}.${{var.dataset}}.{}\"\n    \
               use_table_schema = true\n    \
               write_metadata   = true\n  }}\n  \
             # la bodega tambien necesita DLQ: un mensaje que no encaja en el\n  \
             # esquema no puede desaparecer en silencio\n  \
             dead_letter_policy {{\n    \
               dead_letter_topic     = google_pubsub_topic.{n}_dlq.id\n    \
               max_delivery_attempts = 5\n  }}\n}}\n",
            t.name, t.table
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
    for c in &p.crons {
        let (sv, n) = (tfname(&c.service), tfname(&c.name.replace('.', "-")));
        // OIDC con la misma cuenta del servicio: la ruta del barrido no puede
        // ser publica. Un endpoint interno que resulta ser abierto es un
        // disparador para cualquiera, y este dispara compensaciones.
        o.push(format!(
            "resource \"google_cloud_scheduler_job\" \"{sv}_{n}\" {{\n  \
             name     = \"{svc}-{nombre}\"\n  \
             schedule = \"*/{min} * * * *\"\n  \
             # si una pasada tarda mas que el intervalo, esta se corta antes de\n  \
             # que arranque la siguiente\n  \
             attempt_deadline = \"{plazo}s\"\n  \
             retry_config {{\n    retry_count = 1\n  }}\n  \
             http_target {{\n    \
               http_method = \"POST\"\n    \
               uri         = \"${{google_cloud_run_v2_service.{sv}.uri}}{path}\"\n    \
               oidc_token {{\n      \
                 service_account_email = google_service_account.{sv}.email\n      \
                 audience              = google_cloud_run_v2_service.{sv}.uri\n    }}\n  }}\n}}\n",
            svc = c.service,
            nombre = c.name.replace('.', "-"),
            min = c.minutos(),
            plazo = (c.every_ms / 1000).max(30),
            path = c.path,
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
               availability_type = \"{disp}\"\n{conexiones}{respaldo}  }}\n}}\n",
            svc = s.service,
            disp = if s.ha { "REGIONAL" } else { "ZONAL" },
            conexiones = s
                .max_connections
                .map(|n| format!(
                    "    # el tope contra el que `axon verify` hace la aritmetica, aplicado\n    \
                     database_flags {{\n      name  = \"max_connections\"\n      \
                     value = \"{n}\"\n    }}\n"
                ))
                .unwrap_or_default(),
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
    if p.topics.iter().any(|t| t.analytics) {
        o.push(
            "variable \"firehose_role_arn\" {\n  type        = string\n  \
             description = \"Rol que Firehose asume para escribir en el bucket de aterrizaje\"\n}\n\
             variable \"sns_firehose_role_arn\" {\n  type        = string\n  \
             description = \"Rol que SNS asume para entregar a Firehose\"\n}\n\
             \n\
             # El aterrizaje. La bodega carga DESDE aqui con lo suyo —Snowpipe, una\n\
             # tabla externa— porque ese paso vive del lado de la bodega, no del\n\
             # proveedor. Lo que axon garantiza es que los eventos LLEGUEN, con el\n\
             # mismo particionado por fecha que el esquema generado.\n\
             resource \"aws_s3_bucket\" \"bodega\" {\n  \
             bucket = \"${var.project}-axon-bodega\"\n}\n\
             \n\
             resource \"aws_s3_bucket_versioning\" \"bodega\" {\n  \
             bucket = aws_s3_bucket.bodega.id\n  \
             versioning_configuration { status = \"Enabled\" }\n}\n"
                .to_string(),
        );
    }
    if !p.crons.is_empty() {
        o.push(
            "variable \"scheduler_role_arn\" {\n  type        = string\n  \
             description = \"Rol que EventBridge Scheduler asume para lanzar la tarea del barrido\"\n}\n\
             variable \"ecs_cluster_arn\" {\n  type        = string\n  \
             description = \"ARN del cluster ECS. El scheduler necesita el ARN, no el nombre, y \
             armarlo con region y cuenta obligaria a declarar dos variables que ya suelen existir del lado de quien despliega\"\n}\n\
             variable \"task_execution_role_arn\" {\n  type        = string\n  \
             description = \"Rol de ejecucion de la tarea del barrido\"\n}\n"
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
    for c in &p.crons {
        let (sv, n) = (tfname(&c.service), tfname(&c.name.replace('.', "-")));
        // EventBridge Scheduler no alcanza un endpoint privado: una API
        // destination tendria que ser publica, y la ruta del barrido no puede
        // serlo. Asi que lanza una tarea de un disparo en las MISMAS subredes,
        // que es desde donde el servicio si es alcanzable.
        o.push(format!(
            "resource \"aws_ecs_task_definition\" \"{sv}_{n}\" {{\n  \
             family                   = \"{svc}-{nombre}\"\n  \
             requires_compatibilities = [\"FARGATE\"]\n  \
             network_mode             = \"awsvpc\"\n  cpu = \"256\"\n  memory = \"512\"\n  \
             execution_role_arn       = var.task_execution_role_arn\n  \
             container_definitions    = jsonencode([{{\n    \
               name    = \"barrer\"\n    \
               image   = \"curlimages/curl:8.11.1\"\n    \
               command = [\"-fsS\", \"-m\", \"{plazo}\", \"-X\", \"POST\", \"http://{svc}.internal:{port}{path}\"]\n  \
             }}])\n}}\n",
            svc = c.service,
            nombre = c.name.replace('.', "-"),
            plazo = (c.every_ms / 1000).max(30),
            port = c.port,
            path = c.path,
        ));
        o.push(format!(
            "resource \"aws_scheduler_schedule\" \"{sv}_{n}\" {{\n  \
             name                         = \"{svc}-{nombre}\"\n  \
             schedule_expression          = \"{rate}\"\n  \
             # sin esto, una ventana perdida se recupera disparando varias veces\n  \
             # seguidas: varios barridos a la vez sobre las mismas sagas\n  \
             flexible_time_window {{\n    mode = \"OFF\"\n  }}\n  \
             target {{\n    \
               arn      = var.ecs_cluster_arn\n    \
               role_arn = var.scheduler_role_arn\n    \
               ecs_parameters {{\n      \
                 task_definition_arn = aws_ecs_task_definition.{sv}_{n}.arn\n      \
                 launch_type         = \"FARGATE\"\n      \
                 network_configuration {{\n        subnets = var.subnets\n      }}\n    }}\n    \
               retry_policy {{\n      maximum_retry_attempts = 1\n    }}\n  }}\n}}\n",
            svc = c.service,
            nombre = c.name.replace('.', "-"),
            rate = c.rate(),
        ));
    }
    for t in p.topics.iter().filter(|t| t.analytics) {
        let n = tfname(&t.event);
        // Un stream por evento, y no uno para todo: el particionado y el
        // esquema son por evento, y un solo stream obligaria a separarlos
        // despues, en la bodega, con una consulta que nadie escribio.
        o.push(format!(
            "resource \"aws_kinesis_firehose_delivery_stream\" \"bodega_{n}\" {{\n  \
             name        = \"axon-bodega-{tabla}\"\n  \
             destination = \"extended_s3\"\n  \
             extended_s3_configuration {{\n    \
               role_arn   = var.firehose_role_arn\n    \
               bucket_arn = aws_s3_bucket.bodega.arn\n    \
               # el mismo particionado por fecha que `PARTITION BY DATE(event_time)`\n    \
               # del esquema generado: si no coinciden, la bodega lee de mas\n    \
               prefix              = \"eventos/{tabla}/dt=!{{timestamp:yyyy-MM-dd}}/\"\n    \
               error_output_prefix = \"errores/{tabla}/dt=!{{timestamp:yyyy-MM-dd}}/\"\n    \
               # lo que no encaja no se descarta: cae en `errores/` y se puede\n    \
               # volver a cargar. Es el equivalente del DLQ para la bodega.\n    \
               compression_format  = \"GZIP\"\n    \
               buffering_interval  = 60\n    \
               buffering_size      = 5\n  }}\n}}\n",
            tabla = t.table
        ));
        o.push(format!(
            "resource \"aws_sns_topic_subscription\" \"bodega_{n}\" {{\n  \
             topic_arn             = aws_sns_topic.{n}.arn\n  \
             protocol              = \"firehose\"\n  \
             endpoint              = aws_kinesis_firehose_delivery_stream.bodega_{n}.arn\n  \
             subscription_role_arn = var.sns_firehose_role_arn\n  \
             # el envelope entero, no una envoltura de SNS alrededor: la tabla\n  \
             # generada espera los campos del evento en la raiz\n  \
             raw_message_delivery  = true\n}}\n"
        ));
    }
    for s in &p.stores {
        let sv = tfname(&s.service);
        if let Some(n) = s.max_connections {
            // en RDS el tope no es un atributo de la instancia: va en un
            // parameter group, y sin el la instancia usa el default del motor
            o.push(format!(
                "resource \"aws_db_parameter_group\" \"{sv}\" {{\n  \
                 name   = \"{}-axon\"\n  family = \"postgres16\"\n  \
                 # el tope contra el que `axon verify` hace la aritmetica, aplicado\n  \
                 parameter {{\n    name  = \"max_connections\"\n    value = \"{n}\"\n    \
                 apply_method = \"pending-reboot\"\n  }}\n}}\n",
                s.service
            ));
        }
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
             skip_final_snapshot     = false\n{grupo}}}\n",
            svc = s.service,
            eng = s.engine,
            ha = s.ha,
            ret = s.backup_retention_days,
            grupo = if s.max_connections.is_some() {
                format!("  parameter_group_name    = aws_db_parameter_group.{sv}.name\n")
            } else {
                String::new()
            },
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
        // El barrido de una saga entra por HTTP, asi que si la politica lo deja
        // fuera el CronJob se aplica sin error y no llega nunca: el barrido no
        // corre y lo unico que lo dice es el historial de un job que falla.
        let barrido = p.crons.iter().any(|c| c.service == w.service);
        let mut froms = String::new();
        if desde_edge {
            froms.push_str(
                "\n    - from:\n        - namespaceSelector:\n            matchLabels: { axon.dev/edge: \"true\" }",
            );
        }
        if barrido {
            froms.push_str(
                "\n    # solo el pod del barrido\n    - from:\n        - podSelector:\n            matchLabels: { axon.dev/barrido: \"true\" }",
            );
        }
        let reglas = if !froms.is_empty() {
            froms.as_str()
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
    if p.topics.iter().any(|t| t.analytics) {
        // The ingest path for a cluster. A cluster brings no managed
        // warehouse, so there is nothing to subscribe with: this is the
        // consumer, and its config is generated by `axon analytics --vector`
        // and checked with `vector validate`.
        o.push(
            "---\n\
             apiVersion: apps/v1\n\
             kind: Deployment\n\
             metadata:\n  \
             name: axon-warehouse\n\
             spec:\n  \
             # Several replicas are safe: the NATS queue group delivers each\n  \
             # event once. Without the group every replica would write the same\n  \
             # row and the funnel would count every flow as many times as there\n  \
             # are replicas.\n  \
             replicas: 1\n  \
             selector:\n    \
             matchLabels: { app: axon-warehouse }\n  \
             template:\n    \
             metadata:\n      \
             labels: { app: axon-warehouse }\n    \
             spec:\n      \
             containers:\n        \
             - name: vector\n          \
             image: timberio/vector:0.44.0-alpine\n          \
             args: [--config, /etc/vector/vector.yaml]\n          \
             env:\n            \
             - name: AXON_BROKER_URL\n              \
             value: nats://broker:4222\n            \
             # The warehouse is outside the cluster and its credentials are\n            \
             # not in this file: a generated manifest is no place for them.\n            \
             - name: AXON_WAREHOUSE_URL\n              \
             valueFrom: { secretKeyRef: { name: axon-warehouse, key: url } }\n            \
             - name: AXON_WAREHOUSE_USER\n              \
             valueFrom: { secretKeyRef: { name: axon-warehouse, key: user } }\n            \
             - name: AXON_WAREHOUSE_PASSWORD\n              \
             valueFrom: { secretKeyRef: { name: axon-warehouse, key: password } }\n            \
             - name: AXON_PII_SALT\n              \
             valueFrom: { secretKeyRef: { name: axon-warehouse, key: pii-salt } }\n          \
             volumeMounts:\n            \
             - { name: config, mountPath: /etc/vector, readOnly: true }\n            \
             # The sink buffers on disk: in memory, a restart loses whatever\n            \
             # had not been written yet, and nothing says so.\n            \
             - { name: buffer, mountPath: /var/lib/vector }\n          \
             securityContext:\n            \
             runAsNonRoot: true\n            \
             allowPrivilegeEscalation: false\n            \
             capabilities: { drop: [ALL] }\n      \
             volumes:\n        \
             - name: config\n          \
             configMap: { name: axon-warehouse }\n        \
             - name: buffer\n          \
             emptyDir: {}\n\
             # The config is NOT an object here on purpose. An empty ConfigMap\n\
             # with this name would overwrite the real one the first time\n\
             # someone applied this file, and Vector would come back up with\n\
             # nothing to read. Create it from the generated config:\n\
             #\n\
             #   axon analytics manifests/ --vector > vector.yaml\n\
             #   kubectl create configmap axon-warehouse --from-file=vector.yaml\n\
             #\n\
             # And the secret it reads its credentials from:\n\
             #\n\
             #   kubectl create secret generic axon-warehouse \\\n\
             #     --from-literal=url=... --from-literal=user=... \\\n\
             #     --from-literal=password=... --from-literal=pii-salt=..."
                .to_string(),
        );
    }
    for c in &p.crons {
        let nombre = c.name.replace('.', "-");
        o.push(format!(
            "---
apiVersion: batch/v1
kind: CronJob
metadata:
  name: {svc}-{nombre}
spec:
  schedule: \"*/{min} * * * *\"
  # `Forbid`: si una pasada tarda mas que el intervalo, la siguiente NO arranca.
  # Dos barridos a la vez reclaman la misma saga, y aunque `reclamar` lo evita,
  # no hay razon para apoyarse en eso desde el programador.
  concurrencyPolicy: Forbid
  # el historial es lo unico que queda de un barrido que fallo
  successfulJobsHistoryLimit: 3
  failedJobsHistoryLimit: 3
  jobTemplate:
    spec:
      # sin esto un job atascado se reintenta para siempre
      backoffLimit: 2
      activeDeadlineSeconds: {plazo}
      template:
        metadata:
          # la NetworkPolicy del servicio deja entrar exactamente a esto
          labels: {{ axon.dev/barrido: \"true\" }}
        spec:
          restartPolicy: Never
          containers:
            - name: barrer
              image: curlimages/curl:8.11.1
              args:
                - -fsS
                - -m
                - \"{plazo}\"
                - -X
                - POST
                - http://{svc}.$(NAMESPACE).svc.cluster.local:{port}{path}
              env:
                - name: NAMESPACE
                  valueFrom: {{ fieldRef: {{ fieldPath: metadata.namespace }} }}
              securityContext:
                runAsNonRoot: true
                allowPrivilegeEscalation: false
                readOnlyRootFilesystem: true
                capabilities: {{ drop: [ALL] }}",
            svc = c.service,
            min = c.minutos(),
            plazo = (c.every_ms / 1000).max(30),
            port = c.port,
            path = c.path,
        ));
    }
    o.join("\n")
}

/// El mismo plan, en tu laptop. Ese es el punto: local y produccion salen de
/// la misma declaracion, asi que no pueden divergir.
/// El puerto del host para un servicio en el target local.
///
/// Sale del NOMBRE, no de la posicion. Con el indice, agregar un servicio le
/// movia el puerto a otro que ya estaba —el compose se levanta igual y quien
/// tenia `localhost:8080` en un script apunta de golpe a otro servicio—. Con el
/// nombre, un servicio nuevo no toca los que ya existen.
///
/// Las colisiones se resuelven en orden alfabetico, que es estable: si dos
/// nombres caen en el mismo puerto, el segundo se corre, y sigue haciendolo
/// igual en la proxima corrida.
fn puertos(base: u16, rango: u16, nombres: &[&str]) -> IndexMap<String, u16> {
    let mut orden: Vec<&str> = nombres.to_vec();
    orden.sort_unstable();
    let mut out: IndexMap<String, u16> = IndexMap::new();
    for n in orden {
        // FNV-1a: dos lineas, sin dependencia, y suficiente para repartir
        // nombres cortos en un rango de cientos.
        let mut h: u32 = 2_166_136_261;
        for b in n.as_bytes() {
            h = (h ^ *b as u32).wrapping_mul(16_777_619);
        }
        let mut puerto = base + (h % rango as u32) as u16;
        while out.values().any(|p| *p == puerto) {
            puerto = base + (puerto + 1 - base) % rango;
        }
        out.insert(n.to_string(), puerto);
    }
    out
}

/// Los motores detras de un store: uno solo, o los nodos del sharder. El
/// nombre del contenedor es tambien el host que ve pgdog, asi que sale de aca
/// y no de dos lados.
fn nodos(s: &Store) -> Vec<(u32, String)> {
    match s.shards {
        None => vec![(0, nodo(&s.service, None, 0))],
        Some(n) => (0..n).map(|i| (i, nodo(&s.service, Some(n), i))).collect(),
    }
}

/// El nombre del contenedor de un motor en el target local. Vive aca y lo usa
/// tambien `axon pooler --target local`: el pgdog.toml tiene que nombrar
/// exactamente los hosts que el compose levanta, y dos funciones que
/// concatenan lo mismo se desincronizan en el primer cambio.
pub fn nodo(svc: &str, shards: Option<u32>, i: u32) -> String {
    match shards {
        None => format!("db-{svc}"),
        Some(_) => format!("db-{svc}-{i}"),
    }
}

fn local(p: &Plan) -> String {
    const PROY: &str = "local";
    // Puertos derivados del nombre: agregar un servicio no le mueve el puerto a
    // ninguno de los que ya estaban.
    let svcs: Vec<&str> = p.workloads.iter().map(|w| w.service.as_str()).collect();
    let apps = puertos(8080, 400, &svcs);
    let nodos_todos: Vec<String> = p
        .stores
        .iter()
        .flat_map(|s| nodos(s).into_iter().map(|(_, h)| h))
        .collect();
    let motores = puertos(
        15432,
        400,
        &nodos_todos.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );
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
    for s in p.stores.iter() {
        let svc = &s.service;
        let v = tfname(&s.service);
        let conexiones = s.max_connections.unwrap_or(100);
        for (nodo, host) in nodos(s) {
            let port = motores[&host];
            let sufijo = match s.shards {
                Some(_) => format!("_{nodo}"),
                None => String::new(),
            };
            o.push_str(&format!(
                "  {host}:
    image: postgres:16-alpine
    # el mismo tope que en produccion: agotar conexiones en local es la unica
    # forma de descubrirlo antes de que escale
    command: [\"postgres\", \"-c\", \"max_connections={conexiones}\"]
    environment: {{ POSTGRES_DB: {svc}, POSTGRES_PASSWORD: local }}
    ports: [\"${{AXON_DB_PORT_{v}{sufijo}:-{port}}}:5432\"]
    healthcheck:
      test: [\"CMD-SHELL\", \"pg_isready -U postgres\"]
      interval: 2s
  migrate-{host}:
    image: flyway/flyway:10-alpine
    depends_on: {{ {host}: {{ condition: service_healthy }} }}
    volumes: [\"./sql/{svc}:/flyway/sql:ro\"]
    command: >
      -url=jdbc:postgresql://{host}:5432/{svc}
      -user=postgres -password=local -connectRetries=10
      -sqlMigrationPrefix= -sqlMigrationSeparator=_
      -validateMigrationNaming=true
      migrate
"
            ));
            // Las politicas van en su propio job y su propio historial: se
            // aplican DESPUES del esquema, y regenerarlas no es un cambio de
            // esquema que haya que versionar contra el mismo historial.
            if s.policies {
                o.push_str(&format!(
                    "  policies-{host}:
    image: flyway/flyway:10-alpine
    depends_on: {{ migrate-{host}: {{ condition: service_completed_successfully }} }}
    volumes: [\"./sql-policies/{svc}:/flyway/sql:ro\"]
    # `baselineOnMigrate`: este historial es el SEGUNDO sobre un esquema que ya
    # tiene tablas, las creo el otro. Sin eso Flyway se niega a inicializarse
    # sobre un esquema no vacio, que es la situacion normal aca. Y el comentario
    # va ACA y no dentro del bloque `>`: ahi dentro un `#` es texto, y termina
    # siendo un argumento de Flyway.
    command: >
      -url=jdbc:postgresql://{host}:5432/{svc}
      -user=postgres -password=local -connectRetries=10
      -table=axon_policies_history
      -baselineOnMigrate=true
      -sqlMigrationPrefix= -sqlMigrationSeparator=_
      -validateMigrationNaming=true
      migrate
"
                ));
            }
        }
        // El pooler solo tiene sentido si hay algo detras y ya migrado: pgdog
        // parsea la consulta contra el esquema, y contra una base vacia no
        // sabe a que nodo mandarla.
        if s.shards.is_some() {
            let port = 16432 + (motores[&nodos(s)[0].1] - 15432);
            let ultimo = if s.policies { "policies" } else { "migrate" };
            let espera: Vec<String> = nodos(s)
                .iter()
                .map(|(_, h)| format!("{ultimo}-{h}: {{ condition: service_completed_successfully }}"))
                .collect();
            o.push_str(&format!(
                "  pooler-{svc}:
    # Fijado por digest: pgdog solo publica el tag `main`, que se mueve. Un tag
    # movil en un archivo generado cambia el binario sin cambiar el diff.
    image: ghcr.io/pgdogdev/pgdog:main@sha256:{PGDOG}
    depends_on: {{ {espera} }}
    # el workdir de la imagen es /pgdog, de ahi lee su configuracion
    volumes: [\"./.axon/pgdog/{svc}:/pgdog:ro\"]
    ports: [\"${{AXON_POOLER_PORT_{v}:-{port}}}:6432\"]
    healthcheck:
      # contra el pooler, no contra un nodo: comprueba que pgdog acepta el
      # protocolo, que es lo unico que el servicio va a ver
      test: [\"CMD-SHELL\", \"PGPASSWORD=local psql -h 127.0.0.1 -p 6432 -U postgres -d {svc} -c 'select 1' >/dev/null\"]
      interval: 2s
      retries: 30
",
                espera = espera.join(", ")
            ));
        }
    }
    // los servicios tuyos, no solo sus dependencias
    // Jaeger all-in-one acepta OTLP directo, asi que el backend de trazas es
    // un contenedor y no un colector mas un almacen.
    if p.flags {
        o.push_str(
            "  flags:
    image: ghcr.io/open-feature/flagd:v0.12.9
    command: [\"start\", \"--uri\", \"file:/etc/flags/flags.json\"]
    # 8016 es OFREP, el protocolo REST estandar de OpenFeature
    ports: [\"${AXON_FLAGS_PORT:-8016}:8016\"]
    volumes: [\"./.axon/flags.json:/etc/flags/flags.json:ro\"]
",
        );
    }
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
      # solo lo que declara `traefik.enable`: si no, intenta rutear tambien los
      # jobs de migracion y llena el log de \"port is missing\"
      - --providers.docker.exposedByDefault=false
      - --entrypoints.web.address=:80
    ports: [\"${AXON_EDGE_PORT:-8000}:80\"]
    volumes: [\"/var/run/docker.sock:/var/run/docker.sock:ro\"]
",
        );
    }
    for w in p.workloads.iter() {
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
        // Con pooler, la app NO ve los nodos: ve pgdog. Si el DATABASE_URL
        // apuntara a un nodo, el reparto se saltaria y en local todo
        // funcionaria — con una cuarta parte de los datos.
        let store = p.stores.iter().find(|s| s.service == *svc);
        let db_env = match (w.db, store.and_then(|s| s.shards)) {
            (false, _) => String::new(),
            (true, Some(_)) => {
                deps.push(format!("pooler-{svc}: {{ condition: service_healthy }}"));
                format!("      DATABASE_URL: postgres://postgres:local@pooler-{svc}:6432/{svc}\n")
            }
            (true, None) => {
                deps.push(format!("db-{svc}: {{ condition: service_healthy }}"));
                let ultimo = match store.is_some_and(|s| s.policies) {
                    true => "policies",
                    false => "migrate",
                };
                deps.push(format!(
                    "{ultimo}-db-{svc}: {{ condition: service_completed_successfully }}"
                ));
                format!("      DATABASE_URL: postgres://postgres:local@db-{svc}:5432/{svc}\n")
            }
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
        if p.flags {
            secrets.push_str("      AXON_FLAGS_URL: http://flags:8016\n");
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
            host = apps[&w.service],
            port = w.port,
            v = tfname(&w.service),
            labels = etiquetas_edge(p, &w.service)
        ));
    }
    for (i, c) in p.crons.iter().enumerate() {
        let svc = &c.service;
        // Local no simula el programador del proveedor: hace lo mismo que el,
        // que es golpear la ruta cada tanto. Asi el barrido corre tambien aca y
        // no se descubre en produccion que la ruta no existia.
        o.push_str(&format!(
            "  cron-{n}:
    image: curlimages/curl:8.11.1
    depends_on: {{ {svc}: {{ condition: service_started }} }}
    # `while` y no `sleep` de una vez: un job que corre una sola vez y termina
    # deja a `up --wait` contando un contenedor caido
    command: [\"sh\", \"-c\", \"while :; do sleep {seg}; curl -fsS -m 10 -X POST http://{svc}:{port}{path} || true; done\"]
",
            n = tfname(&c.name.replace('.', "-")),
            seg = (c.every_ms / 1000).max(1),
            port = c.port,
            path = c.path,
        ));
        let _ = i;
    }
    if p.topics.iter().any(|t| t.analytics) {
        // La bodega en local no es un adorno: sin ella el esquema se genera y
        // nadie comprueba nunca que las columnas y las rutas del JSON
        // coincidan. Se llena del log de envelopes que este mismo target ya
        // escribe, asi que la traza y la analitica salen de la misma fuente.
        o.push_str(
            "  bodega:\n    \
             image: clickhouse/clickhouse-server:24.8-alpine\n    \
             environment: { CLICKHOUSE_DB: axon, CLICKHOUSE_USER: local, CLICKHOUSE_PASSWORD: local }\n    \
             # El log de envelopes, donde `file()` puede leerlo. NO read-only: el\n    \
             # entrypoint de la imagen hace `chown` de este directorio y con `:ro`\n    \
             # falla y el contenedor no arranca.\n    \
             volumes: [\"./.axon:/var/lib/clickhouse/user_files\"]\n    \
             ports: [\"${AXON_BODEGA_PORT:-8123}:8123\"]\n    \
             healthcheck:\n      \
             test: [\"CMD-SHELL\", \"wget -qO- http://127.0.0.1:8123/ping\"]\n      \
             interval: 2s\n      \
             retries: 30\n",
        );
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
