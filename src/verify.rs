//! Drift: lo que convierte al manifiesto en algo mas que documentacion.
use crate::manifest::*;
use indexmap::IndexMap;

/// Gobernanza: reglas del equipo, versionadas junto al codigo.
/// Sin `axon.policy.toml` aplican estos valores por defecto.
#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct Policy {
    pub require_owner: bool,
    pub require_tier: bool,
    pub allowed_event_prefixes: Vec<String>,
    pub max_deps_per_service: usize,
    /// El layout del repo es del equipo, no de axon. Sin esto, `axon ci`
    /// tendria que adivinarlo, y adivinar es lo que lo volvia inservible.
    pub ci: Ci,
}

/// `{service}` se reemplaza por el nombre del servicio.
#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct Ci {
    pub manifests_dir: String,
    pub service_dir: String,
    pub test_cmd: String,
    pub contracts_path: String,
    pub image: String,
}

impl Default for Ci {
    fn default() -> Self {
        Self {
            manifests_dir: "manifests".into(),
            service_dir: "services/{service}".into(),
            test_cmd: "make -C services/{service} test".into(),
            contracts_path: "services/{service}/src/contracts.ts".into(),
            image: "${{ vars.REGISTRY }}/{service}@${{ steps.imagen.outputs.digest }}".into(),
        }
    }
}

impl Ci {
    pub fn para(&self, campo: &str, service: &str) -> String {
        campo.replace("{service}", service)
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            require_owner: true,
            require_tier: true,
            allowed_event_prefixes: vec![],
            max_deps_per_service: 7, // acoplamiento sincrono: si pasa de esto, es un monolito distribuido
            ci: Ci::default(),
        }
    }
}

pub fn load_policy(dir: &std::path::Path) -> Policy {
    let f = dir.join("axon.policy.toml");
    std::fs::read_to_string(f)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Heuristica corta y deliberadamente conservadora: solo formas que no pueden
/// ser el nombre de una variable de entorno.
fn parece_secreto(v: &str) -> bool {
    let t = v.trim();
    t.starts_with("sk_")
        || t.starts_with("AKIA")
        || t.starts_with("ghp_")
        || t.starts_with("-----BEGIN")
        || (t.len() > 32 && t.chars().any(|c| c.is_lowercase()) && t.contains(['/', '+', '=']))
}

/// Un placeholder no es un valor. Sin esto, `axon import` produciria
/// manifiestos que pasan verify sin que nadie los haya revisado.
fn pendiente(v: &Option<String>) -> bool {
    match v {
        None => true,
        Some(s) => {
            let s = s.trim().to_uppercase();
            s.is_empty() || s == "TODO" || s == "FIXME" || s == "TBD"
        }
    }
}

pub fn verify(ms: &[Manifest], pol: &Policy) -> Report {
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());

    // governance: nothing without an owner, nothing without a criticality,
    // names under control
    for m in ms.iter().filter(|m| !m.external) {
        if pol.require_owner && pendiente(&m.owner) {
            errors.push(format!(
                "{}: no `owner`; a service with no owner does not get deployed",
                m.service
            ));
        }
        if pol.require_tier && pendiente(&m.tier) {
            errors.push(format!(
                "{}: no `tier`; criticality decides alerts and SLOs",
                m.service
            ));
        }
        match m.infra.runtime.as_deref() {
            None | Some("container") => {}
            Some(other) => errors.push(format!(
                "{}: `runtime = \"{other}\"` does not exist; today there is only `container`. \
                 Another execution model gets added with an `axon-infra-*` plugin",
                m.service
            )),
        }
        if m.depends.len() > pol.max_deps_per_service {
            warnings.push(format!(
                "{}: {} synchronous dependencies (limit {}); check whether something should \
                 be an event",
                m.service,
                m.depends.len(),
                pol.max_deps_per_service
            ));
        }
        if !pol.allowed_event_prefixes.is_empty() {
            for ev in m.emits.keys() {
                let pre = ev.split('.').next().unwrap_or("");
                if !pol.allowed_event_prefixes.iter().any(|p| p == pre) {
                    errors.push(format!(
                        "{ev}: prefix `{pre}` is outside the allowed domain catalogue",
                    ));
                }
            }
        }
    }

    // one event, one owner, one schema
    let mut emitters: IndexMap<&str, (&str, &Fields)> = IndexMap::new();
    for m in ms {
        for (ev, fields) in &m.emits {
            if let Some((owner, prev)) = emitters.get(ev.as_str()) {
                if *prev != fields {
                    errors.push(format!(
                        "{ev}: two emitters with different schemas ({owner} vs {})",
                        m.service
                    ));
                }
            }
            emitters.insert(ev, (&m.service, fields));
        }
    }

    // ---- security. Every rule cites its OWASP Top 10 (2021) category,
    // because an error that does not say why it matters gets silenced with an
    // allow-list entry.
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        let pii = &m.pii;

        for (name, meth) in &m.methods {
            let public = meth.auth.as_deref() == Some("public");
            // A01: broken access control. A public mutating route on a critical
            // service is not a decision anyone takes without meaning to.
            if public && meth.mutating() && m.tier.as_deref() == Some("0") {
                errors.push(format!(
                    "[A01] {svc}.{name}: public mutating route on a tier 0 service; put it \
                     behind `auth = \"required\"` or lower the tier deliberately"
                ));
            }
            // A04: insecure design. With no time budget, a public route is free
            // resource exhaustion.
            if public && meth.timeout_ms.is_none() {
                errors.push(format!(
                    "[A04] {svc}.{name}: public route with no `timeout_ms`; a request with no \
                     time limit is resource exhaustion"
                ));
            }
            // A09: logging failures. Personal data that leaves through a public
            // route ends up in a log, a cache and a CDN.
            if public {
                for field in meth.output.keys() {
                    if es_pii(pii, field) {
                        errors.push(format!(
                            "[A09] {svc}.{name}: returns `{field}`, declared PII, through a \
                             public route"
                        ));
                    }
                }
            }
        }

        // A02: cryptographic failures. A secret in the manifest is already in
        // git history; declaring its name is the only thing that belongs here.
        for k in &m.infra.secrets {
            if parece_secreto(k) {
                errors.push(format!(
                    "[A02] {svc}: `{k}` in `secrets` looks like the value and not the name; \
                     the reference goes there, the value lives in the vault"
                ));
            }
        }

        // A05: security misconfiguration. A public bucket with no retention
        // grows forever and nobody knows what is inside it.
        for (name, b) in &m.infra.buckets {
            if b.public && b.retention_days.is_none() {
                warnings.push(format!(
                    "[A05] {svc}: bucket `{name}` is public and has no `retention_days`; \
                     nobody will know what was left exposed"
                ));
            }
        }
    }

    // A01: a table that forgets the tenant column gets no policy, and a table
    // with no policy does not fail: it returns everyone's rows.
    let esquemas = schemas(ms);
    for m in ms.iter().filter(|m| !m.external) {
        let Some(tenant) = &m.infra.tenant_column else {
            continue;
        };
        let Some(tables) = esquemas.get(&m.service) else {
            continue;
        };
        for (t, cols) in tables {
            if ["outbox", "inbox_seen"].contains(&t.as_str()) || m.infra.tenant_exempt.contains(t) {
                continue;
            }
            if !cols.tiene(tenant) {
                errors.push(format!(
                    "[A01] {}.{t}: no `{tenant}` column; it ends up with no RLS policy and \
                     returns rows from every tenant. Add it, or list the table in \
                     `tenant_exempt`",
                    m.service
                ));
            }
        }
    }

    // ---- export to the warehouse ----
    for m in ms.iter().filter(|m| !m.external) {
        if !BODEGAS.contains(&m.analytics.warehouse.as_str()) {
            errors.push(format!(
                "{}: `[analytics] warehouse = \"{}\"` has no dialect. Available: {}",
                m.service,
                m.analytics.warehouse,
                BODEGAS.join(", ")
            ));
        }
        match m.analytics.pii.as_str() {
            "exclude" | "hash" => {}
            other => errors.push(format!(
                "{}: `[analytics] pii = \"{other}\"` does not exist; use \"exclude\" or \"hash\"",
                m.service
            )),
        }
        // Exporting hashed personal data is still exporting it: the hash of an
        // email identifies the same person across two different tables.
        if m.analytics.pii == "hash" && m.analytics.export && !m.pii.is_empty() {
            warnings.push(format!(
                "{}: exports {} hashed personal fields to the warehouse. A hash is not \
                 anonymisation: it identifies the same person across tables, so it works \
                 for counting and for joining alike",
                m.service,
                m.pii.len()
            ));
        }
    }

    // ---- feature flags: what nobody enforces ----
    let ahora = hoy();
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        for (nombre, f) in &m.flags {
            if f.owner.is_none() {
                errors.push(format!(
                    "{svc}.{nombre}: flag with no `owner`. Whoever turned it on is who turns it off"
                ));
            }
            // A codebase with two hundred old flags does not have two hundred
            // features: it has two hundred branches nobody tests.
            match (&f.expires, f.kill_switch) {
                (None, false) => errors.push(format!(
                    "{svc}.{nombre}: flag with no `expires`. A flag with no death date does not die; \
                     if it really is permanent, declare it `kill_switch = true`"
                )),
                (Some(_), true) => warnings.push(format!(
                    "{svc}.{nombre}: `kill_switch` with `expires`; an emergency switch lives as long as \
                     the thing it turns off does"
                )),
                (Some(e), false) => match fecha(e) {
                    None => errors.push(format!(
                        "{svc}.{nombre}: `expires = \"{e}\"` is not in YYYY-MM-DD form"
                    )),
                    Some(f) if f < ahora => errors.push(format!(
                        "{svc}.{nombre}: expired on {e}. Either the dead branch gets cleaned up or the date \
                         gets renewed as an explicit decision: leaving it expired is neither"
                    )),
                    _ => {}
                },
                _ => {}
            }

            // Un rollout por peticion hace que la MISMA entidad tome un camino
            // en una llamada y el otro en la siguiente. Con estado de por
            // medio, eso deja datos a medio migrar.
            // el orden importa: un kill switch con rollout es un error propio,
            // no un caso del sticky que falta
            match f.rollout {
                Some(_) if f.kill_switch => errors.push(format!(
                    "{svc}.{nombre}: `kill_switch` with `rollout`. An emergency switch turns everything \
                     off or it is worth nothing"
                )),
                Some(p) if p > 100 => errors.push(format!(
                    "{svc}.{nombre}: `rollout = {p}` is not a percentage"
                )),
                Some(p) if p > 0 && p < 100 && f.sticky_by.is_none() => errors.push(format!(
                    "{svc}.{nombre}: rollout at {p}% with no `sticky_by`. Evaluated per request, the same \
                     entity takes one path and then the other, and ends up half-migrated"
                )),
                _ => {}
            }
            // A default variant that does not exist makes evaluation
            // caiga siempre al valor del codigo, y el flag deja de servir en
            // silencio: se ve como "el rollout no hace nada".
            let variantes = f.variantes();
            let defecto = f.variante_defecto();
            if !variantes.contains_key(&defecto) {
                errors.push(format!(
                    "{svc}.{nombre}: `default_variant = \"{defecto}\"` is not in `variants` ({}). \
                     Evaluation would always fall back to the value in the code",
                    variantes.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            // Mixing types across variants breaks evaluation: OpenFeature
            // resuelve un tipo por flag, no uno por variante.
            let tipos: std::collections::BTreeSet<&str> = variantes
                .values()
                .map(|v| match v {
                    serde_json::Value::Bool(_) => "boolean",
                    serde_json::Value::String(_) => "string",
                    serde_json::Value::Number(_) => "number",
                    _ => "object",
                })
                .collect();
            if tipos.len() > 1 {
                errors.push(format!(
                    "{svc}.{nombre}: the variants mix types ({}). OpenFeature resolves one type per \
                     flag, not one per variant",
                    tipos.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
            if f.default && !f.kill_switch {
                warnings.push(format!(
                    "{svc}.{nombre}: `default = true` on a flag that is not a kill switch. A new flag on \
                     by default is not a gradual rollout: it is a deploy"
                ));
            }
            // El campo por el que se fija tiene que existir en algun contrato,
            // o la decision se fija por un dato que el servicio no recibe.
            if let Some(campo) = &f.sticky_by {
                let conocido = m.infra.tenant_column.as_deref() == Some(campo.as_str())
                    || m.methods.values().any(|me| me.input.contains_key(campo))
                    || m.emits.values().any(|fs| fs.contains_key(campo))
                    || ms.iter().any(|o| {
                        m.consumes
                            .keys()
                            .any(|ev| o.emits.get(ev).is_some_and(|fs| fs.contains_key(campo)))
                    });
                if !conocido {
                    errors.push(format!(
                        "{svc}.{nombre}: se fija por `{campo}`, que no aparece en ningun contrato \
                         ni es la columna del inquilino; el servicio no lo recibe"
                    ));
                }
            }
        }
    }

    // ---- the pooler: it changes the subject of the arithmetic, and it can
    // break tenant isolation without raising an error ----
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        let pl = &m.pooler;
        if !pl.activo() {
            // declaring pooler fields with no pooler is configuration that
            // lands nowhere
            if pl.shards > 1 || pl.max_client_conn.is_some() || pl.tenant_binding.is_some() {
                errors.push(format!(
                    "{svc}: there are `[pooler]` fields declared with `engine = \"none\"`; none \
                     of them is applied anywhere"
                ));
            }
            continue;
        }
        match pl.engine.as_str() {
            "pgdog" => {}
            otro => errors.push(format!(
                "{svc}: `[pooler] engine = \"{otro}\"` is not supported. Native: pgdog"
            )),
        }
        match pl.mode.as_str() {
            "transaction" | "session" | "statement" => {}
            otro => errors.push(format!(
                "{svc}: `[pooler] mode = \"{otro}\"` does not exist; use transaction, session or statement"
            )),
        }

        // THE RULE. In transaction mode the connection goes back to the pool at
        // every COMMIT and is handed to another tenant. If the tenant is pinned
        // with a session `SET`, the value survives and the next request reads
        // the previous tenant's rows. With no error.
        if m.infra.tenant_column.is_some() && pl.mode != "session" {
            match pl.tenant_binding.as_deref() {
                Some("set_local") => {}
                _ => errors.push(format!(
                    "{svc}: `mode = \"{}\"` with `tenant_column` and no `tenant_binding = \
                     \"set_local\"`. The connection goes back to the pool at every COMMIT and is \
                     handed to another tenant: a session GUC survives and the next request reads \
                     the previous tenant's rows, with no error. `SET LOCAL` dies with the \
                     transaction",
                    pl.mode
                )),
            }
        }
        if let Some(b) = &pl.tenant_binding {
            if b != "set_local" {
                errors.push(format!(
                    "{svc}: `tenant_binding = \"{b}\"` does not exist; the only safe one is \"set_local\""
                ));
            }
        }

        // Sharding without declaring which column shards by is not possible.
        if pl.shards > 1 && m.infra.shard_key.is_none() {
            errors.push(format!(
                "{svc}: `shards = {}` with no `shard_key`. The sharder needs to know which \
                 column it shards by, and `verify` needs to check that every table carries it",
                pl.shards
            ));
        }

        // A sharder's 2PC gives eventual consistency with visible partial reads,
        // not atomicity. Promising CP on top is the same class of contradiction
        // as reading from a replica and promising CP.
        if pl.shards > 1 && !m.cap.eventual() {
            errors.push(format!(
                "{svc}: {} shard nodes with `consistency = \"strong\"`. A transaction that crosses \
                 nodes commits in two phases and makes partial states visible: the real \
                 guarantee is eventual, and declaring it strong does not change that",
                pl.shards
            ));
        }

        // Measured against pgdog: with the tenant column declared, EVERY query on
        // a table that carries it has to filter by it or the router rejects it
        // with `no multi tenant id`. And it is the same for the sharder: without
        // the key it does not know which node to go to. So a method that does
        // not receive the tenant cannot be served — and the symptom shows up on
        // the first request against a real pooler, not in the manifest.
        if let (true, Some(col)) = (pl.shards > 1, m.infra.tenant_column.as_ref()) {
            for (nombre, me) in m.methods.iter() {
                if me.input.keys().any(|k| normalizar(k) == normalizar(col)) {
                    continue;
                }
                errors.push(format!(
                    "{svc}.{nombre}: does not receive `{col}` and the database is sharded by \
                     that column. The router rejects a query that does not filter by tenant \
                     (`no multi tenant id`), and the sharder does not know which node to send \
                     it to. Add it to `in`, and usually to the route as well"
                ));
            }
        }

        // With the pooler in front, the subject of the arithmetic changes twice.
        let techo = m.infra.max_instances.unwrap_or(10);
        if let (Some(pool), Some(clientes)) = (m.infra.pool_size, pl.max_client_conn) {
            let pico = pool * techo;
            if pico > clientes {
                errors.push(format!(
                    "{svc}: {pool} connections x {techo} instances = {pico} clients, and the \
                     pooler accepts {clientes}. With a pooler in between, the arithmetic runs \
                     against its client limit, not the engine's"
                ));
            }
        }
        if let (Some(ppool), Some(tope)) = (pl.pool_size, m.infra.max_connections) {
            // el pooler abre ESTE tanto a CADA motor, y los nodos y las
            // replicas son motores distintos, cada uno con su propio tope
            let reservado = if m.patterns.outbox { 5 } else { 2 };
            if ppool + reservado > tope {
                errors.push(format!(
                    "{svc}: the pooler opens {ppool} connections to each engine, plus \
                     {reservado} reserved, against a limit of {tope} PER ENGINE"
                ));
            }
        }
        // In session mode one client connection pins one server connection: there
        // is no multiplexing, so declaring more clients than engine connections
        // promises something the pooler does not do.
        if pl.mode == "session" {
            if let (Some(pool), Some(ppool)) = (m.infra.pool_size, pl.pool_size) {
                if pool * techo > ppool {
                    errors.push(format!(
                        "{svc}: `mode = \"session\"` does not multiplex —one client connection \
                         pins one server connection—, and {pool} x {techo} = {} clients against \
                         {ppool} engine connections",
                        pool * techo
                    ));
                }
            }
        }

        // A query that crosses nodes, once executed, can return an incomplete
        // result in silence: no cross-node JOIN, no global uniqueness.
        if pl.shards > 1 && !pl.cross_shard_disabled {
            warnings.push(format!(
                "{svc}: `cross_shard_disabled = false` with {} nodes. A query that crosses \
                 nodes runs anyway, and the ones the sharder cannot resolve —cross-node JOINs, \
                 window functions, aggregates not on its list— can return an incomplete result \
                 instead of an error",
                pl.shards
            ));
        }
    }

    // ---- el motor tiene que existir ----
    for m in ms.iter().filter(|m| !m.external) {
        let Some(motor) = &m.infra.state else {
            continue;
        };
        if !MOTORES.contains(&motor.as_str()) {
            errors.push(format!(
                "{}: `state = \"{motor}\"` no esta soportado. Motores nativos: {}. Un motor \
                 distinto se resuelve con un plugin `axon-infra-{motor}`, que recibe el plan \
                 neutral por stdin; sin eso, axon generaria infraestructura de Postgres para \
                 algo que no lo es",
                m.service,
                MOTORES.join(", ")
            ));
        }
    }

    // ---- database scaling: arithmetic over what was declared ----
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        let inf = &m.infra;
        if inf.state.is_none() {
            if inf.pool_size.is_some() || inf.read_replicas.is_some() {
                errors.push(format!(
                    "{svc}: declares a pool or replicas with no `state`; it has no database of its own"
                ));
            }
            continue;
        }

        // Connection exhaustion does not show up when you test with one
        // instance: it shows up the day it scales. It is a multiplication, and
        // nobody does it.
        //
        // With a pooler in between, the instances do not open connections to
        // the engine: the pooler does. The subject of the multiplication
        // changes, and that arithmetic belongs to the [pooler] rules. Without
        // this exception the two rules contradict each other, and one of them
        // advises adding a pooler that is already there.
        if let (Some(pool), Some(tope), false) =
            (inf.pool_size, inf.max_connections, m.pooler.activo())
        {
            let techo = inf.max_instances.unwrap_or(10);
            let pico = pool * techo;
            // the outbox relay and the migrations open connections too
            let reservado = if m.patterns.outbox { 5 } else { 2 };
            if pico + reservado > tope {
                errors.push(format!(
                    "{svc}: {pool} connections x {techo} instances = {pico}, plus {reservado} \
                     reserved, exceeds the limit of {tope}. The service falls over from \
                     exhaustion when it scales, not when you test it: lower the pool, lower \
                     max_instances, or put a pooler in front"
                ));
            } else if pico * 2 > tope {
                warnings.push(format!(
                    "{svc}: at peak it uses {pico} of {tope} connections; that leaves little room for \
                     migrations, a pooler or a second service on the same instance"
                ));
            }
        }
        if inf.pool_size.is_some() != inf.max_connections.is_some() {
            warnings.push(format!(
                "{svc}: declares `pool_size` or `max_connections` but not the other; without both \
                 there is no way to check for exhaustion"
            ));
        }

        // A tier 0 with no failover is not a tier 0: it is a statement of intent
        // with nothing behind it.
        let tier0 = m.tier.as_deref() == Some("0");
        if tier0 && inf.ha != Some(true) {
            errors.push(format!(
                "{svc}: `tier = \"0\"` with no `ha = true`. A critical service with a single \
                 database instance goes down with it: either declare the standby or lower \
                 the tier"
            ));
        }
        // High availability is not a backup: a standby replicates the DROP TABLE
        // en segundos. Son dos problemas distintos con dos soluciones distintas.
        match inf.backup_retention_days {
            None if tier0 => errors.push(format!(
                "{svc}: `tier = \"0\"` with no `backup_retention_days`. High availability is not a \
                 backup: the standby replicates a delete within seconds"
            )),
            Some(d) if d < 7 && tier0 => errors.push(format!(
                "{svc}: {d} days of backups on a tier 0. A logical delete gets discovered after \
                 the weekend, not a minute later"
            )),
            _ => {}
        }
        if inf.pitr == Some(true) && inf.backup_retention_days.is_none() {
            errors.push(format!(
                "{svc}: `pitr` with no `backup_retention_days`. Point-in-time recovery needs a base \
                 backup to roll forward from"
            ));
        }
        if inf.ha.is_some() && inf.state.is_none() {
            errors.push(format!(
                "{svc}: declares `ha` with no `state`; it has no database of its own"
            ));
        }

        // Una replica va con retraso. Leer de ella y prometer consistencia
        // fuerte es la contradiccion del teorema, escrita en dos lugares.
        if inf.read_replicas.unwrap_or(0) > 0 && !m.cap.eventual() {
            errors.push(format!(
                "{svc}: reads from {} replicas and declares `consistency = \"strong\"`. A replica \
                 lags: either the reads are `eventual`, or they do not come from there",
                inf.read_replicas.unwrap_or(0)
            ));
        }
    }

    // ---- sharding across nodes ----
    //
    // These rules hold against plain Postgres with sharding in the application,
    // which is how most of the people who really shard do it. And nobody
    // enforces them: PgDog's schema validator is on its roadmap and not
    // started, and Citus only fails at runtime when distributing the table.
    // Every one of them describes a leak or a collision that raises no error,
    // just wrong data.
    let esquemas_shard = schemas(ms);
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        let Some(clave) = &m.infra.shard_key else {
            continue;
        };
        let Some(tablas) = esquemas_shard.get(svc) else {
            continue;
        };

        // Isolating by one column and sharding by another makes every query from
        // one tenant touch every node: the sharding stops being worth anything.
        if let Some(inq) = &m.infra.tenant_column {
            if inq != clave {
                errors.push(format!(
                    "{svc}: isolates by `{inq}` and shards by `{clave}`. Every query from one \
                     tenant would touch every node, so the sharding buys nothing"
                ));
            }
        }

        // N nodes are N timelines: there is no global recovery point. Restoring
        // to an instant leaves the transactions that crossed nodes cut in half.
        if m.infra.pitr == Some(true) {
            errors.push(format!(
                "{svc}: `pitr = true` with `shard_key`. Each node has its own timeline: there \
                 is no consistent recovery point for the set, and restoring leaves the \
                 transactions that crossed nodes cut in half"
            ));
        }

        let repartidas: Vec<&String> = tablas
            .iter()
            .filter(|(_, t)| t.tiene(clave))
            .map(|(t, _)| t)
            .collect();

        for (t, tb) in tablas {
            if ["outbox", "inbox_seen"].contains(&t.as_str()) || m.infra.tenant_exempt.contains(t) {
                continue;
            }
            if !repartidas.contains(&t) {
                errors.push(format!(
                    "{svc}.{t}: no `{clave}` column, so it cannot be sharded. Add it, or \
                     take the table out of the sharded schema"
                ));
                continue;
            }

            // Each node satisfies a UNIQUE locally; the set does not. If the
            // constraint does not include the shard key, two nodes can accept
            // the same value and nobody raises an error.
            for u in &tb.uniques {
                // A uuid is unique by construction everywhere, so each node
                // satisfying it separately IS ENOUGH. Without this exception the
                // rule flags every uuid PK and turns into noise — and a rule
                // with false positives gets silenced.
                let global =
                    u.len() == 1 && tb.col(&u[0]).is_some_and(|c| c.ty.starts_with("uuid"));
                if global {
                    continue;
                }
                if !u.iter().any(|c| c == clave) {
                    errors.push(format!(
                        "{svc}.{t}: `UNIQUE ({})` does not include `{clave}`. Each node \
                         satisfies it separately and the set does not: two nodes accept the \
                         same value with no error. Add the key to the constraint, or the \
                         uniqueness is an illusion",
                        u.join(", ")
                    ));
                }
            }

            // Each node has its own sequence, starting at 1.
            for c in tb.cols.iter().filter(|c| c.serial) {
                errors.push(format!(
                    "{svc}.{t}.{}: generated from a sequence (`{}`) in a sharded schema. \
                     Each node has its own and the values collide: use a uuid, or a generator \
                     that carries the node inside it",
                    c.name, c.ty
                ));
            }

            for c in &tb.cols {
                let Some(fk) = &c.fk else { continue };
                if tablas.contains_key(fk) && !repartidas.contains(&fk) {
                    errors.push(format!(
                        "{svc}.{t}.{}: FK to `{fk}`, which does not carry `{clave}`. A FK \
                         between a sharded table and one that is not crosses nodes, and that \
                         cannot be guaranteed",
                        c.name
                    ));
                }
            }
        }
    }

    // ---- CAP: the partition is not a choice, what to do during one is ----
    let lado: IndexMap<&str, &Cap> = ms.iter().map(|m| (m.service.as_str(), &m.cap)).collect();
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        let cap = &m.cap;
        if !cap.declarado {
            warnings.push(format!(
                "{svc}: no `[cap]`; assumed CP (strong/reject), which fails closed. \
                 Declare it so the choice belongs to someone and not to the default"
            ));
        }
        match cap.consistency.as_str() {
            "strong" | "eventual" => {}
            otro => errors.push(format!(
                "{svc}: `consistency = \"{otro}\"` does not exist; use \"strong\" or \"eventual\""
            )),
        }
        match cap.on_partition.as_str() {
            "reject" | "degrade" => {}
            otro => errors.push(format!(
                "{svc}: `on_partition = \"{otro}\"` does not exist; use \"reject\" or \"degrade\""
            )),
        }
        // "eventual" with no number is a word, not a guarantee
        if cap.eventual() && cap.max_staleness_ms.is_none() {
            errors.push(format!(
                "{svc}: `consistency = \"eventual\"` with no `max_staleness_ms`; with no \
                 staleness budget nobody can say whether the data it served was acceptable"
            ));
        }
        // you cannot be CP and serve something stale: that is the theorem's own
        // contradiction
        if !cap.eventual() && cap.degrada() {
            errors.push(format!(
                "{svc}: `strong` with `on_partition = \"degrade\"` contradicts itself; \
                 serving stale data IS choosing availability over consistency"
            ));
        }
        // your guarantee is that of the weakest link on the synchronous path
        for d in &m.depends {
            if !cap.eventual() && lado.get(d.target()).is_some_and(|c| c.eventual()) {
                warnings.push(format!(
                    "{svc} is `strong` and calls {}, which is `eventual`: the guarantee of \
                     the path is the weaker one, not yours",
                    d.target()
                ));
            }
        }
        // deciding with strong consistency from an input that does not have it
        for (nombre, mac) in &m.machine {
            for (act, t) in &mac.transitions {
                if !cap.eventual() && m.consumes.contains_key(&t.on) {
                    warnings.push(format!(
                        "{svc}.{nombre}.{act}: `strong` transition triggered by the event \
                         `{}`, which arrives eventually; the state may have changed first",
                        t.on
                    ));
                }
            }
        }
    }

    // A08: integrity failures. A tag is mutable: what gets deployed today is
    // not what was audited yesterday.
    if pol.ci.image.contains(":latest") || !pol.ci.image.contains('@') {
        warnings.push(format!(
            "[A08] [ci].image `{}` does not pin a digest; a tag is mutable and the deploy \
             stops being reproducible",
            pol.ci.image
        ));
    }

    // patrones de API: lo que separa un endpoint de uno que aguanta produccion
    let mut routes: IndexMap<String, String> = IndexMap::new();
    for m in ms.iter().filter(|m| !m.external) {
        for (name, meth) in &m.methods {
            let Some(http) = &meth.http else { continue };
            if let Some(prev) = routes.insert(http.clone(), m.service.clone()) {
                errors.push(format!("`{http}` declarada por {prev} y por {}", m.service));
            }
            match meth.path() {
                Some(p) if !p.starts_with("/v") => errors.push(format!(
                    "{}.{name}: `{p}` sin version en la ruta; usa /v1/...",
                    m.service
                )),
                _ => {}
            }
            if meth.mutating() && !meth.idempotent {
                errors.push(format!(
                    "{}.{name}: {http} muta sin `idempotent = true`; un reintento del cliente \
                     duplicaria el efecto",
                    m.service
                ));
            }
            // El gateway falla cerrado: una ruta expuesta sin decidir quien
            // puede llamarla no se despliega. No hay default seguro para esto.
            match meth.auth.as_deref() {
                Some("public") | Some("required") => {}
                Some(otro) => errors.push(format!(
                    "{}.{name}: `auth = \"{otro}\"` no existe; usa \"public\" o \"required\"",
                    m.service
                )),
                None => errors.push(format!(
                    "{}.{name}: {http} expuesta sin `auth`; declara \"public\" o \"required\"",
                    m.service
                )),
            }
            if meth.auth.as_deref() == Some("public") && meth.rate_limit.is_none() {
                errors.push(format!(
                    "{}.{name}: {http} es publica y sin `rate_limit`; el edge no tiene \
                     con que frenar un abuso",
                    m.service
                ));
            }
            if meth.paginated && !meth.output.contains_key("cursor") {
                errors.push(format!(
                    "{}.{name}: paginada pero no devuelve `cursor`; offset se rompe al crecer",
                    m.service
                ));
            }
        }
    }

    // Una bodega por plataforma. Los eventos de un mismo flujo tienen que caer
    // en el mismo lugar: repartidos entre dos bodegas, el embudo —que es lo que
    // hace util exportar— no se puede armar con una sola consulta, y nadie ve
    // un error porque cada tabla existe y tiene filas.
    let exportan: Vec<&Manifest> = ms
        .iter()
        .filter(|m| !m.external && m.analytics.export)
        .collect();
    if let Some(primero) = exportan.first() {
        for otro in exportan.iter().skip(1) {
            if otro.analytics.warehouse != primero.analytics.warehouse {
                errors.push(format!(
                    "{} exporta a `{}` y {} a `{}`. Los eventos de un mismo flujo tienen que \
                     caer en la misma bodega o el embudo no se puede armar, y cada tabla \
                     existiria con filas sin que nada avise",
                    primero.service,
                    primero.analytics.warehouse,
                    otro.service,
                    otro.analytics.warehouse
                ));
            }
        }
    }

    let known: IndexMap<&str, &Manifest> = ms.iter().map(|m| (m.service.as_str(), m)).collect();

    // sagas: un paso sin compensacion no es una saga, es un dual-write con
    // mas pasos y mas formas de quedarse a medias
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        for (nombre, sg) in &m.saga {
            if sg.steps.is_empty() {
                errors.push(format!("{svc}.{nombre}: una saga sin pasos no coordina nada"));
                continue;
            }

            // El disparador tiene que existir: una saga que nadie arranca es
            // codigo generado que nunca corre.
            match &sg.on {
                None => errors.push(format!(
                    "{svc}.{nombre}: sin `on`. Una saga la arranca un metodo propio o un \
                     evento consumido, y sin decir cual el coordinador se genera y nunca corre"
                )),
                Some(on) => {
                    if !m.methods.contains_key(on) && !m.consumes.contains_key(on) {
                        errors.push(format!(
                            "{svc}.{nombre}: la arranca `{on}`, que no es un metodo de `{svc}` \
                             ni un evento que consuma"
                        ));
                    }
                }
            }

            // El avance tiene que estar en disco. Un coordinador que pierde la
            // saga a medias no la termina ni la compensa: los pasos ya hechos
            // quedan aplicados para siempre y nadie sabe cuales fueron.
            let tabla = Saga::tabla(nombre);
            match esquemas.get(svc) {
                None => errors.push(format!(
                    "{svc}.{nombre}: sin migraciones, y la saga necesita la tabla `{tabla}` para \
                     sobrevivir a un reinicio del coordinador"
                )),
                Some(tablas) => match tablas.get(&tabla) {
                    None => errors.push(format!(
                        "{svc}.{nombre}: falta la tabla `{tabla}`. Sin ella un reinicio a mitad \
                         de la saga deja los pasos ya hechos aplicados y sin registro de cuales \
                         fueron: no se puede terminar ni compensar"
                    )),
                    Some(t) => {
                        // `datos` y `actualizado` no son adorno: sin el envelope
                        // que la arranco no se puede reconstruir la llamada al
                        // retomar, y sin la marca de tiempo el barrido no puede
                        // distinguir una saga colgada de una que va en camino.
                        for (col, para) in [
                            ("id", "el id del flujo"),
                            ("paso", "hasta donde llego"),
                            ("estado", "si el paso se intento o se completo"),
                            ("datos", "el envelope que la arranco, para poder retomarla"),
                            ("actualizado", "cuando avanzo por ultima vez, para el barrido"),
                        ] {
                            match t.col(col) {
                                None => errors.push(format!(
                                    "{svc}.{nombre}: `{tabla}` sin columna `{col}`: ahi va {para}"
                                )),
                                Some(c) => {
                                    // Un tipo equivocado no da error: da una
                                    // comparacion que compila y compara mal.
                                    let esperado = match col {
                                        "datos" => "json",
                                        "actualizado" => "timestamp",
                                        _ => continue,
                                    };
                                    if !c.ty.to_lowercase().contains(esperado) {
                                        errors.push(format!(
                                            "{svc}.{nombre}: `{tabla}.{col}` es `{}` y tiene que \
                                             ser {esperado}. Comparar una fecha guardada como \
                                             texto compila y ordena mal: el barrido se saltaria \
                                             sagas colgadas sin decir nada",
                                            c.ty
                                        ));
                                    }
                                }
                            }
                        }
                    }
                },
            }

            let ultimo = sg.steps.len() - 1;
            let mut presupuesto = 0u32;
            for (i, paso) in sg.steps.iter().enumerate() {
                // Cada referencia se resuelve contra los manifiestos, no contra
                // la buena fe: un `undo` mal escrito es una compensacion que no
                // existe, y se descubre el dia que hay que compensar.
                let mut resolver = |campo: &str, r: &str| -> Option<(u32, &Method)> {
                    let Some((s, met)) = Paso::partes(r) else {
                        errors.push(format!(
                            "{svc}.{nombre}.{campo}: `{r}` no tiene la forma `servicio.metodo`"
                        ));
                        return None;
                    };
                    let Some(otro) = known.get(s) else {
                        errors.push(format!(
                            "{svc}.{nombre}.{campo}: `{r}` apunta a `{s}`, que no existe"
                        ));
                        return None;
                    };
                    let Some(me) = otro.methods.get(met) else {
                        errors.push(format!("{svc}.{nombre}.{campo}: `{s}` no ofrece `{met}`"));
                        return None;
                    };
                    // El paso se invoca con el cliente generado, y ese cliente
                    // existe solo si la dependencia esta declarada. Sin eso la
                    // saga se genera y no tiene con que llamar.
                    let dep = m
                        .depends
                        .iter()
                        .find(|d| d.service.as_deref() == Some(s) && d.method == met);
                    if s != svc.as_str() && dep.is_none() {
                        errors.push(format!(
                            "{svc}.{nombre}.{campo}: usa `{r}` sin declararlo en `[[depends]]`. \
                             El cliente resiliente —timeout, reintentos, breaker— sale de ahi, y \
                             sin el la saga no tiene con que llamar"
                        ));
                    }
                    // El presupuesto del paso es el del LLAMADOR, no el que el
                    // otro servicio declara para si, y con los reintentos
                    // dentro: el coordinador espera lo que dice `[[depends]]`.
                    let unitario = dep.and_then(|d| d.timeout_ms).or(me.timeout_ms).unwrap_or(0);
                    let intentos = dep.map(|d| d.retries + 1).unwrap_or(1);
                    Some((unitario * intentos, me))
                };

                if let Some((ms, _)) = resolver("do", &paso.hacer) {
                    presupuesto += ms;
                }

                match &paso.undo {
                    // Carve-out deliberado: si el ULTIMO paso falla, no hay
                    // nada suyo que deshacer. Exigirle compensacion seria un
                    // falso positivo, y una regla con falsos positivos se
                    // silencia entera.
                    None if i == ultimo => {}
                    None => errors.push(format!(
                        "{svc}.{nombre}: el paso {} (`{}`) no tiene `undo`, y no es el ultimo. \
                         Si falla un paso posterior, este queda aplicado para siempre: eso no es \
                         una saga, es un dual-write con mas pasos",
                        i + 1,
                        paso.hacer
                    )),
                    Some(u) => {
                        if let Some((ms, me)) = resolver("undo", u) {
                            // La compensacion se reintenta hasta que entra: no
                            // hay nada detras de ella. Una que no es idempotente
                            // aplica el efecto dos veces.
                            if !me.idempotent {
                                errors.push(format!(
                                    "{svc}.{nombre}: `{u}` compensa el paso {} y no es \
                                     `idempotent`. Una compensacion se reintenta hasta que entra \
                                     —no hay nada detras— y reintentar la que no es idempotente \
                                     aplica el efecto dos veces",
                                    i + 1
                                ));
                            }
                            presupuesto += ms;
                        }
                        if *u == paso.hacer {
                            errors.push(format!(
                                "{svc}.{nombre}: el paso {} se compensa consigo mismo",
                                i + 1
                            ));
                        }
                    }
                }
            }

            // La misma clase de aritmetica que la de las conexiones, y el mismo
            // error de fondo: un numero declarado que no cubre la suma de los
            // que ya estaban declarados.
            match sg.timeout_ms {
                Some(tope) if presupuesto > tope => errors.push(format!(
                    "{svc}.{nombre}: `timeout_ms = {tope}` y los pasos mas sus compensaciones \
                     suman {presupuesto}ms. Rendirse mientras un paso sigue en vuelo deja al \
                     coordinador compensando algo que despues tiene exito"
                )),
                Some(_) => {}
                None => warnings.push(format!(
                    "{svc}.{nombre}: sin `timeout_ms`. Una saga sin presupuesto de tiempo se \
                     queda en vuelo hasta que alguien la mira"
                )),
            }

            // Una saga es consistencia eventual por construccion: entre el
            // primer paso y el ultimo el sistema pasa por estados que ningun
            // invariante describe. Prometer CP encima es la misma contradiccion
            // que leer de una replica y prometer CP.
            if !m.cap.eventual() {
                errors.push(format!(
                    "{svc}.{nombre}: coordina una saga con `consistency = \"strong\"`. Entre el \
                     primer paso y el ultimo hay estados intermedios visibles que ningun \
                     invariante describe: la garantia real del flujo es eventual"
                ));
            }
        }
    }

    // event sourcing: el flujo es la verdad, asi que lo que se refuta es todo
    // lo que lo convierte en algo que no es un flujo
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        for (nombre, ag) in &m.aggregate {
            if ag.events.is_empty() {
                errors.push(format!(
                    "{svc}.{nombre}: un agregado sin eventos no tiene estado que reconstruir"
                ));
            }
            // Un agregado fundado en un evento que el servicio no emite es un
            // agregado que nadie puede llenar.
            for ev in &ag.events {
                if !m.emits.contains_key(ev) {
                    errors.push(format!(
                        "{svc}.{nombre}: se funda en `{ev}`, que este servicio no declara emitir. \
                         El flujo lo escribe su dueno: si el evento es de otro, esto es una vista, \
                         no un agregado"
                    ));
                }
            }
            // La maquina, si se declara, tiene que existir y hablar de los
            // mismos eventos: dos vocabularios para el mismo concepto se
            // separan en el primer cambio.
            if let Some(mac) = &ag.machine {
                match m.machine.get(mac) {
                    None => errors.push(format!(
                        "{svc}.{nombre}: gobernado por `[machine.{mac}]`, que no existe"
                    )),
                    Some(maq) => {
                        for ev in &ag.events {
                            if !maq.transitions.values().any(|t| t.emits.as_ref() == Some(ev)) {
                                errors.push(format!(
                                    "{svc}.{nombre}: `{ev}` es del agregado y ninguna transicion \
                                     de `{mac}` lo emite. El `fold` generado no sabria a que \
                                     estado llevarlo"
                                ));
                            }
                        }
                    }
                }
            }

            // Los eventos del agregado salen al bus, y el flujo ya es durable:
            // publicar en linea despues de anotar deja una ventana en la que el
            // evento esta en el flujo y nadie lo recibio. Y publicar ANTES de
            // anotar es peor. El traspaso tiene que ser durable y en la misma
            // transaccion que el append, y eso es el outbox.
            if !ag.events.is_empty() && !m.patterns.outbox {
                errors.push(format!(
                    "{svc}.{nombre}: un agregado cuyos eventos se publican necesita \
                     `[patterns] outbox = true`. El flujo ya es durable, asi que publicar en \
                     linea deja una ventana en la que el evento esta anotado y nadie lo recibio, \
                     y publicar antes de anotar deja lo contrario. El traspaso va en la MISMA \
                     transaccion que el append"
                ));
            }

            let tabla = Aggregate::tabla(nombre);
            match esquemas.get(svc).and_then(|t| t.get(&tabla)) {
                None => errors.push(format!(
                    "{svc}.{nombre}: falta la tabla `{tabla}`. El estado ES el flujo, y sin la \
                     tabla no hay donde ponerlo"
                )),
                Some(t) => {
                    for (col, para) in [
                        ("stream_id", "de que instancia del agregado es este evento"),
                        ("version", "su posicion en el flujo"),
                        ("type", "cual de los eventos declarados es"),
                        ("data", "su contenido"),
                    ] {
                        if !t.tiene(col) {
                            errors.push(format!(
                                "{svc}.{nombre}: `{tabla}` sin columna `{col}`: ahi va {para}"
                            ));
                        }
                    }
                    // Sin el UNIQUE, dos escrituras concurrentes sobre el mismo
                    // flujo se aceptan las dos con la misma version. Nadie ve un
                    // error y el estado reconstruido depende del orden de
                    // lectura.
                    let optimista = t.uniques.iter().any(|u| {
                        u.len() == 2
                            && u.iter().any(|c| c == "stream_id")
                            && u.iter().any(|c| c == "version")
                    });
                    if !optimista {
                        errors.push(format!(
                            "{svc}.{nombre}: `{tabla}` sin UNIQUE sobre (stream_id, version). Dos \
                             escrituras concurrentes sobre el mismo flujo entran las dos con la \
                             misma version, sin un solo error, y el estado que se reconstruye \
                             depende de en que orden se lean"
                        ));
                    }
                }
            }
            // Append-only, y no como recomendacion. Una migracion que
            // actualiza o borra del flujo no rompe nada visible: deja un
            // pasado que no ocurrio, y todo lo que se reconstruya despues sera
            // consistente con esa mentira. No hay `.contract.sql` que lo
            // habilite, a diferencia del resto de las tablas.
            for f in migrations_of(m) {
                let texto = std::fs::read_to_string(&f).unwrap_or_default().to_lowercase();
                let nombre_archivo = f.file_name().unwrap_or_default().to_string_lossy().to_string();
                for verbo in ["update", "delete from", "truncate"] {
                    // se busca el verbo Y la tabla en la misma sentencia
                    for sent in texto.split(';') {
                        let limpio: String = sent
                            .lines()
                            .filter(|l| !l.trim_start().starts_with("--"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if limpio.contains(verbo) && limpio.contains(&tabla) {
                            errors.push(format!(
                                "{svc}/{nombre_archivo}: `{}` sobre `{tabla}`, que es el flujo de \
                                 `{nombre}`. Un flujo es append-only: cambiar un evento pasado \
                                 deja un pasado que no ocurrio, y todo lo que se reconstruya \
                                 despues va a ser coherente con esa mentira. Para corregir se \
                                 agrega un evento nuevo, no se edita el viejo",
                                verbo.to_uppercase()
                            ));
                        }
                    }
                }
            }
            if ag.snapshot_every > 0 {
                let fotos = Aggregate::fotos(nombre);
                match esquemas.get(svc).and_then(|t| t.get(&fotos)) {
                    None => errors.push(format!(
                        "{svc}.{nombre}: `snapshot_every = {}` sin la tabla `{fotos}`",
                        ag.snapshot_every
                    )),
                    Some(t) => {
                        for (col, para, tipo) in [
                            ("stream_id", "de que instancia es la foto", ""),
                            ("version", "hasta que evento del flujo la cubre", ""),
                            ("estado", "el estado calculado", "json"),
                            (
                                "reglas",
                                "con que version de las reglas se calculo: sin esta columna, \
                                 una foto vieja se rehidrata con reglas nuevas y da un estado \
                                 que ya no coincide con reproducir el flujo, sin ningun error",
                                "",
                            ),
                        ] {
                            match t.col(col) {
                                None => errors.push(format!(
                                    "{svc}.{nombre}: `{fotos}` sin columna `{col}`: ahi va {para}"
                                )),
                                Some(c) if !tipo.is_empty() && !c.ty.to_lowercase().contains(tipo) => {
                                    errors.push(format!(
                                        "{svc}.{nombre}: `{fotos}.{col}` es `{}` y tiene que ser \
                                         {tipo}",
                                        c.ty
                                    ))
                                }
                                _ => {}
                            }
                        }
                        // Una foto por cada evento no es una cache, es una
                        // segunda copia del flujo con el doble de escrituras.
                        if ag.snapshot_every == 1 {
                            warnings.push(format!(
                                "{svc}.{nombre}: `snapshot_every = 1` guarda una foto por evento: \
                                 eso no es una cache, es una segunda copia del flujo con el doble \
                                 de escrituras"
                            ));
                        }
                    }
                }
            }
        }

        // Modelos de lectura.
        for (nombre, vi) in &m.view {
            if vi.on.is_empty() {
                errors.push(format!(
                    "{svc}.{nombre}: una vista sin eventos no se construye con nada"
                ));
            }
            for ev in &vi.on {
                // Puede consumir eventos propios o de otro; lo que no puede es
                // consumir uno que nadie emite.
                if !emitters.contains_key(ev.as_str()) {
                    errors.push(format!(
                        "{svc}.{nombre}: se construye con `{ev}`, que nadie emite"
                    ));
                }
                // Y si es de otro servicio, tiene que estar declarado en
                // `[consumes]`: la entrega la arma `axon infra` desde ahi.
                let propio = m.emits.contains_key(ev.as_str());
                if !propio && !m.consumes.contains_key(ev.as_str()) {
                    errors.push(format!(
                        "{svc}.{nombre}: usa `{ev}`, de otro servicio, sin declararlo en \
                         `[consumes]`. La suscripcion sale de ahi, y sin ella la vista se \
                         genera y nunca recibe nada"
                    ));
                }
            }
            let tablas = esquemas.get(svc);
            let tabla = vi.tabla(nombre);
            if tablas.and_then(|t| t.get(&tabla)).is_none() {
                errors.push(format!(
                    "{svc}.{nombre}: falta la tabla `{tabla}`, donde vive la vista"
                ));
            }
            let cp = View::checkpoint(nombre);
            match tablas.and_then(|t| t.get(&cp)) {
                None => errors.push(format!(
                    "{svc}.{nombre}: falta la tabla `{cp}`. Sin anotar hasta donde llego, un \
                     reinicio reprocesa desde el principio o se salta lo que no alcanzo a \
                     aplicar; las dos cosas dan una vista incorrecta y ninguna da un error"
                )),
                Some(t) => {
                    for (col, para) in [
                        ("vista", "cual de las vistas es"),
                        ("stream_id", "de que flujo"),
                        ("posicion", "hasta que version de ESE flujo llego"),
                    ] {
                        if !t.tiene(col) {
                            errors.push(format!(
                                "{svc}.{nombre}: `{cp}` sin columna `{col}`: ahi va {para}"
                            ));
                        }
                    }
                    // Sin `stream_id` en la clave, un flujo pisa el punto de
                    // otro. La version de un evento es su posicion dentro de SU
                    // flujo: un solo numero para toda la vista parece funcionar
                    // mientras haya un flujo, y deja de identificar nada en
                    // cuanto hay dos.
                    let por_flujo = t.uniques.iter().any(|u| {
                        u.iter().any(|c| c == "stream_id") && u.iter().any(|c| c == "vista")
                    });
                    if !por_flujo && t.tiene("stream_id") {
                        errors.push(format!(
                            "{svc}.{nombre}: `{cp}` sin clave sobre (vista, stream_id). Un flujo \
                             pisaria el punto de otro, y la vista se saltaria eventos o los \
                             reprocesaria sin que nada avise"
                        ));
                    }
                }
            }
            // Si la vista se puede reconstruir, hay una sombra, y una sombra
            // con una columna de menos hace que el intercambio deje una vista
            // incompleta. Eso se descubriria el dia de la reconstruccion, que
            // es el peor dia.
            let propia = !vi.on.is_empty()
                && vi
                    .on
                    .iter()
                    .all(|ev| m.aggregate.values().any(|a| a.events.contains(ev)));
            if propia {
                let sombra = format!("{}_sombra", tabla);
                match (
                    tablas.and_then(|t| t.get(&tabla)),
                    tablas.and_then(|t| t.get(&sombra)),
                ) {
                    (Some(_), None) => errors.push(format!(
                        "{svc}.{nombre}: se puede reconstruir y falta `{sombra}`. Reconstruir en \
                         el sitio deja la vista incompleta mientras corre, y se sigue leyendo: \
                         los que preguntan reciben menos filas de las que hay, sin un error"
                    )),
                    (Some(viva), Some(som)) => {
                        for c in &viva.cols {
                            match som.col(&c.name) {
                                None => errors.push(format!(
                                    "{svc}.{nombre}: `{sombra}` sin la columna `{}` que tiene \
                                     `{tabla}`. El intercambio dejaria una vista sin ese dato, y \
                                     recien ahi se veria",
                                    c.name
                                )),
                                Some(o) if o.ty != c.ty => errors.push(format!(
                                    "{svc}.{nombre}: `{sombra}.{}` es `{}` y en `{tabla}` es \
                                     `{}`. Al intercambiar, la vista cambia de tipo sin que nada \
                                     lo diga",
                                    c.name, o.ty, c.ty
                                )),
                                _ => {}
                            }
                        }
                        for c in &som.cols {
                            if !viva.tiene(&c.name) {
                                warnings.push(format!(
                                    "{svc}.{nombre}: `{sombra}.{}` no esta en `{tabla}`. Sobra \
                                     hasta el proximo intercambio, y despues es la vista la que \
                                     la tiene",
                                    c.name
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Una vista es eventual por construccion: se llena DESPUES de que
            // el evento ocurrio. Prometer CP sobre ella es la misma
            // contradiccion que leer de una replica y prometer CP.
            if !m.cap.eventual() {
                errors.push(format!(
                    "{svc}.{nombre}: un modelo de lectura con `consistency = \"strong\"`. La \
                     vista se llena despues de que el evento ocurrio: lo que sirve es un dato \
                     viejo por definicion"
                ));
            }
            match (vi.max_staleness_ms, m.cap.max_staleness_ms) {
                (Some(v), Some(tope)) if v > tope => errors.push(format!(
                    "{svc}.{nombre}: la vista admite {v}ms de atraso y el servicio declaro un \
                     tope de {tope}ms. El servicio no puede cumplir lo que prometio sirviendo \
                     de una vista mas vieja que su propio presupuesto"
                )),
                (None, _) => warnings.push(format!(
                    "{svc}.{nombre}: sin `max_staleness_ms`. Sin presupuesto de atraso, nadie \
                     puede decir si la vista que sirvio estaba aceptablemente vieja"
                )),
                _ => {}
            }
        }
    }

    // maquinas de estado: estados muertos, inalcanzables y disparadores fantasma
    for m in ms.iter().filter(|m| !m.external) {
        for (name, mac) in &m.machine {
            let states = mac.states();
            if !states.contains(&mac.initial) {
                errors.push(format!(
                    "{}.{name}: estado inicial `{}` no aparece en ninguna transicion",
                    m.service, mac.initial
                ));
            }
            // alcanzabilidad desde el inicial
            let mut reach = vec![mac.initial.clone()];
            let mut grew = true;
            while grew {
                grew = false;
                for t in mac.transitions.values() {
                    if t.from.iter().any(|f| reach.contains(f)) && !reach.contains(&t.to) {
                        reach.push(t.to.clone());
                        grew = true;
                    }
                }
            }
            for st in &states {
                if !reach.contains(st) {
                    errors.push(format!(
                        "{}.{name}: estado `{st}` inalcanzable desde `{}`",
                        m.service, mac.initial
                    ));
                }
                let sale = mac.transitions.values().any(|t| t.from.contains(st));
                if !sale && !mac.final_states.contains(st) {
                    errors.push(format!(
                        "{}.{name}: `{st}` no es final y no tiene salida; es un deadlock",
                        m.service
                    ));
                }
            }
            for (act, t) in &mac.transitions {
                if !states.contains(&t.to) {
                    errors.push(format!(
                        "{}.{name}.{act}: destino `{}` desconocido",
                        m.service, t.to
                    ));
                }
                // el disparador tiene que existir de verdad
                if !m.methods.contains_key(&t.on) && !m.consumes.contains_key(&t.on) {
                    errors
                        .push(format!(
                        "{}.{name}.{act}: la dispara `{}`, que no es ni metodo ni evento consumido",
                        m.service, t.on));
                }
                if let Some(ev) = &t.emits {
                    if !m.emits.contains_key(ev) {
                        errors.push(format!(
                            "{}.{name}.{act}: emite `{ev}`, que el servicio no declara emitir",
                            m.service
                        ));
                    }
                }
                if let Some(c) = &t.compensates {
                    if !mac.transitions.contains_key(c) {
                        errors.push(format!(
                            "{}.{name}.{act}: compensa `{c}`, que no existe",
                            m.service
                        ));
                    }
                }
            }
        }
    }

    for m in ms {
        let svc = &m.service;
        for ev in m.consumes.keys() {
            if !emitters.contains_key(ev.as_str()) {
                errors.push(format!("{svc} consume {ev} pero nadie lo emite"));
            }
        }
        for d in &m.depends {
            let tgt = d.target();
            match known.get(tgt) {
                None => errors.push(format!("{svc} depende de {tgt}, sin manifiesto conocido")),
                Some(t) if !t.methods.contains_key(&d.method) => errors.push(format!(
                    "{svc} llama {tgt}.{}, que {tgt} no expone",
                    d.method
                )),
                _ => {}
            }
            if d.timeout_ms.is_none() {
                errors.push(format!(
                    "{svc} -> {tgt}.{}: sin `timeout_ms`; una llamada sin presupuesto de tiempo \
                     propaga la caida del otro lado",
                    d.method
                ));
            }
            if d.retries > 0
                && !known
                    .get(tgt)
                    .is_some_and(|t| t.methods.get(&d.method).is_some_and(|m| m.is_idempotent()))
            {
                errors.push(format!(
                    "{svc} reintenta {tgt}.{}, que no se declara idempotente",
                    d.method
                ));
            }
            if d.retries > 0 && !d.breaker {
                warnings.push(format!(
                    "{svc} -> {tgt}.{}: reintentos sin `breaker = true`; los reintentos amplifican \
                     la caida del otro lado", d.method));
            }
        }
    }

    // migraciones: expand -> migrate -> contract, y orden determinista
    for m in ms {
        // Dos migraciones con la misma version: Flyway se niega a aplicar
        // NINGUNA, asi que el despliegue se cae con la base a medio migrar. Lo
        // caza al arrancar; aca se caza al commitear, que es cuando se puede
        // renombrar el archivo sin prisa.
        let mut vistas: IndexMap<String, String> = IndexMap::new();
        for f in migrations_of(m) {
            let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
            let Some((ver, _)) = name.split_once('_') else {
                continue;
            };
            if let Some(otra) = vistas.get(ver) {
                errors.push(format!(
                    "{}: `{name}` y `{otra}` comparten la version `{ver}`. Flyway no aplica \
                     ninguna de las dos y el despliegue se cae con la base a medio migrar",
                    m.service
                ));
            } else {
                vistas.insert(ver.to_string(), name);
            }
        }
        for f in migrations_of(m) {
            let name = f
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            if destructive(&text, &f.display().to_string()) && !name.contains(".contract.") {
                errors.push(format!(
                    "{}/{name}: migracion destructiva sin marcar como `.contract.sql` \
                     (expand -> migrate -> contract)",
                    m.service
                ));
            }
            // `001_x.sql`: tres digitos y un guion bajo. Una regex para esto
            // seria una dependencia entera por tres caracteres.
            let numerado = {
                let b = name.as_bytes();
                b.len() > 3 && b[..3].iter().all(u8::is_ascii_digit) && b[3] == b'_'
            };
            if !numerado {
                warnings.push(format!(
                    "{}/{name}: sin prefijo numerico, el orden no es determinista",
                    m.service
                ));
            }
        }
    }

    // database per service: ninguna FK cruza el limite
    let by_svc = schemas(ms);
    let mut owner_of: IndexMap<&str, &str> = IndexMap::new();
    for (svc, tables) in &by_svc {
        for t in tables.keys() {
            owner_of.insert(t, svc);
        }
    }
    for (svc, tables) in &by_svc {
        for (t, cols) in tables {
            for c in &cols.cols {
                let Some(fk) = &c.fk else { continue };
                if let Some(owner) = owner_of.get(fk.as_str()) {
                    if owner != svc {
                        errors.push(format!(
                            "{svc}.{t}.{}: FK a {fk} cruza el limite de servicio \
                             (dueno: {owner}); guarda el id, no una FK",
                            c.name
                        ));
                    }
                }
            }
        }
    }

    for (ev, (owner, _)) in &emitters {
        if !ms.iter().any(|m| m.consumes.contains_key(*ev)) {
            warnings.push(format!("{ev} ({owner}) no tiene consumidores"));
        }
    }
    Report { errors, warnings }
}
