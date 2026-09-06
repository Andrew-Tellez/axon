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

    // API patterns: what separates an endpoint from one that survives production
    let mut routes: IndexMap<String, String> = IndexMap::new();
    for m in ms.iter().filter(|m| !m.external) {
        for (name, meth) in &m.methods {
            let Some(http) = &meth.http else { continue };
            if let Some(prev) = routes.insert(http.clone(), m.service.clone()) {
                errors.push(format!("`{http}` declarada por {prev} y por {}", m.service));
            }
            match meth.path() {
                Some(p) if !p.starts_with("/v") => errors.push(format!(
                    "{}.{name}: `{p}` has no version in the path; use /v1/...",
                    m.service
                )),
                _ => {}
            }
            if meth.mutating() && !meth.idempotent {
                errors.push(format!(
                    "{}.{name}: {http} mutates with no `idempotent = true`; a client retry \
                     would duplicate the effect",
                    m.service
                ));
            }
            // The gateway fails closed: a route exposed without deciding who may
            // call it does not get deployed. There is no safe default here.
            match meth.auth.as_deref() {
                Some("public") | Some("required") => {}
                Some(otro) => errors.push(format!(
                    "{}.{name}: `auth = \"{otro}\"` does not exist; use \"public\" or \"required\"",
                    m.service
                )),
                None => errors.push(format!(
                    "{}.{name}: {http} is exposed with no `auth`; declare \"public\" or \"required\"",
                    m.service
                )),
            }
            if meth.auth.as_deref() == Some("public") && meth.rate_limit.is_none() {
                errors.push(format!(
                    "{}.{name}: {http} is public and has no `rate_limit`; the edge has \
                     nothing to throttle abuse with",
                    m.service
                ));
            }
            if meth.paginated && !meth.output.contains_key("cursor") {
                errors.push(format!(
                    "{}.{name}: paginated but does not return a `cursor`; offset breaks as it grows",
                    m.service
                ));
            }
        }
    }

    // One warehouse per platform. The events of one flow have to land in the
    // same place: split across two warehouses, the funnel —which is what makes
    // exporting worth anything— cannot be built with a single query, and nobody
    // sees an error because every table exists and has rows.
    let exportan: Vec<&Manifest> = ms
        .iter()
        .filter(|m| !m.external && m.analytics.export)
        .collect();
    if let Some(primero) = exportan.first() {
        for otro in exportan.iter().skip(1) {
            if otro.analytics.warehouse != primero.analytics.warehouse {
                errors.push(format!(
                    "{} exports to `{}` and {} to `{}`. The events of one flow have to land \
                     in the same warehouse or the funnel cannot be built, and every table \
                     would exist with rows without anything warning about it",
                    primero.service,
                    primero.analytics.warehouse,
                    otro.service,
                    otro.analytics.warehouse
                ));
            }
        }
    }

    let known: IndexMap<&str, &Manifest> = ms.iter().map(|m| (m.service.as_str(), m)).collect();

    // sagas: a step with no compensation is not a saga, it is a dual-write with
    // more steps and more ways to end up half-done
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        for (nombre, sg) in &m.saga {
            if sg.steps.is_empty() {
                errors.push(format!("{svc}.{nombre}: una saga sin pasos no coordina nada"));
                continue;
            }

            // The trigger has to exist: a saga nobody starts is generated code that
            // never runs.
            match &sg.on {
                None => errors.push(format!(
                    "{svc}.{nombre}: no `on`. A saga is started by a method of its own or by a \
                     consumed event, and without saying which, the coordinator gets generated \
                     and never runs"
                )),
                Some(on) => {
                    if !m.methods.contains_key(on) && !m.consumes.contains_key(on) {
                        errors.push(format!(
                            "{svc}.{nombre}: started by `{on}`, which is neither a method of `{svc}` \
                             nor an event it consumes"
                        ));
                    }
                }
            }

            // The progress has to be on disk. A coordinator that loses a saga
            // halfway neither finishes nor compensates it: the steps already
            // taken stay applied forever and nobody knows which ones they were.
            let tabla = Saga::tabla(nombre);
            match esquemas.get(svc) {
                None => errors.push(format!(
                    "{svc}.{nombre}: no migrations, and the saga needs the `{tabla}` table to \
                     survive a restart of the coordinator"
                )),
                Some(tablas) => match tablas.get(&tabla) {
                    None => errors.push(format!(
                        "{svc}.{nombre}: the `{tabla}` table is missing. Without it, a restart mid-saga \
                         leaves the steps already taken applied and with no record of which \
                         ones: it can neither finish nor compensate"
                    )),
                    Some(t) => {
                        // `datos` and `actualizado` are not decoration: without the
                        // envelope that started it, the call cannot be rebuilt
                        // on resume, and without the timestamp the sweep cannot
                        // tell a stranded saga from one still on its way.
                        for (col, para) in [
                            ("id", "the flow's id"),
                            ("paso", "how far it got"),
                            ("estado", "whether the step was attempted or completed"),
                            ("datos", "the envelope that started it, so it can be resumed"),
                            ("actualizado", "when it last moved, for the sweep"),
                        ] {
                            match t.col(col) {
                                None => errors.push(format!(
                                    "{svc}.{nombre}: `{tabla}` has no `{col}` column: that is where {para} goes"
                                )),
                                Some(c) => {
                                    // A wrong type raises no error: it gives a
                                    // comparison that compiles and compares wrong.
                                    let esperado = match col {
                                        "datos" => "json",
                                        "actualizado" => "timestamp",
                                        _ => continue,
                                    };
                                    if !c.ty.to_lowercase().contains(esperado) {
                                        errors.push(format!(
                                            "{svc}.{nombre}: `{tabla}.{col}` is `{}` and has to \
                                             be {esperado}. Comparing a date stored as text \
                                             compiles and sorts wrong: the sweep would skip \
                                             stranded sagas without saying anything",
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
                // Every reference is resolved against the manifests, not against
                // good faith: a misspelled `undo` is a compensation that does
                // not exist, and it gets discovered the day it is needed.
                let mut resolver = |campo: &str, r: &str| -> Option<(u32, &Method)> {
                    let Some((s, met)) = Paso::partes(r) else {
                        errors.push(format!(
                            "{svc}.{nombre}.{campo}: `{r}` is not in `service.method` form"
                        ));
                        return None;
                    };
                    let Some(otro) = known.get(s) else {
                        errors.push(format!(
                            "{svc}.{nombre}.{campo}: `{r}` points at `{s}`, which does not exist"
                        ));
                        return None;
                    };
                    let Some(me) = otro.methods.get(met) else {
                        errors.push(format!("{svc}.{nombre}.{campo}: `{s}` does not offer `{met}`"));
                        return None;
                    };
                    // The step is invoked with the generated client, and that client
                    // exists only if the dependency is declared. Without it the
                    // saga gets generated with nothing to call with.
                    let dep = m
                        .depends
                        .iter()
                        .find(|d| d.service.as_deref() == Some(s) && d.method == met);
                    if s != svc.as_str() && dep.is_none() {
                        errors.push(format!(
                            "{svc}.{nombre}.{campo}: uses `{r}` without declaring it in \
                             `[[depends]]`. The resilient client —timeout, retries, breaker— \
                             comes from there, and without it the saga has nothing to call with"
                        ));
                    }
                    // The step's budget is the CALLER's, not the one the other
                    // service declares for itself, and with the retries inside:
                    // the coordinator waits for what `[[depends]]` says.
                    let unitario = dep.and_then(|d| d.timeout_ms).or(me.timeout_ms).unwrap_or(0);
                    let intentos = dep.map(|d| d.retries + 1).unwrap_or(1);
                    Some((unitario * intentos, me))
                };

                if let Some((ms, _)) = resolver("do", &paso.hacer) {
                    presupuesto += ms;
                }

                match &paso.undo {
                    // A deliberate carve-out: if the LAST step fails, there is
                    // nothing of its own to undo. Demanding a compensation for
                    // it would be a false positive, and a rule with false
                    // positives gets silenced wholesale.
                    None if i == ultimo => {}
                    None => errors.push(format!(
                        "{svc}.{nombre}: step {} (`{}`) has no `undo`, and it is not the \
                         last one. If a later step fails, this one stays applied forever: that \
                         is not a saga, it is a dual-write with more steps",
                        i + 1,
                        paso.hacer
                    )),
                    Some(u) => {
                        if let Some((ms, me)) = resolver("undo", u) {
                            // The compensation gets retried until it lands: there
                            // is nothing behind it. One that is not idempotent
                            // applies the effect twice.
                            if !me.idempotent {
                                errors.push(format!(
                                    "{svc}.{nombre}: `{u}` compensates step {} and is not \
                                     `idempotent`. A compensation gets retried until it lands \
                                     —there is nothing behind it— and retrying one that is not \
                                     idempotent applies the effect twice",
                                    i + 1
                                ));
                            }
                            presupuesto += ms;
                        }
                        if *u == paso.hacer {
                            errors.push(format!(
                                "{svc}.{nombre}: step {} compensates itself",
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
                    "{svc}.{nombre}: `timeout_ms = {tope}` and the steps plus their \
                     compensations add up to {presupuesto}ms. Giving up while a step is still \
                     in flight leaves the coordinator compensating something that later \
                     succeeds"
                )),
                Some(_) => {}
                None => warnings.push(format!(
                    "{svc}.{nombre}: no `timeout_ms`. A saga with no time budget stays in \
                     flight until somebody looks at it"
                )),
            }

            // Una saga es consistencia eventual por construccion: entre el
            // primer paso y el ultimo el sistema pasa por estados que ningun
            // invariante describe. Prometer CP encima es la misma contradiccion
            // que leer de una replica y prometer CP.
            if !m.cap.eventual() {
                errors.push(format!(
                    "{svc}.{nombre}: coordinates a saga with `consistency = \"strong\"`. \
                     Between the first step and the last there are visible intermediate states \
                     no invariant describes: the real guarantee of the flow is eventual"
                ));
            }
        }
    }

    // event sourcing: the stream is the truth, so what gets refuted is
    // everything that turns it into something that is not a stream
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        for (nombre, ag) in &m.aggregate {
            if ag.events.is_empty() {
                errors.push(format!(
                    "{svc}.{nombre}: an aggregate with no events has no state to rebuild"
                ));
            }
            // An aggregate founded on an event the service does not emit is an
            // aggregate nobody can fill.
            for ev in &ag.events {
                if !m.emits.contains_key(ev) {
                    errors.push(format!(
                        "{svc}.{nombre}: is founded on `{ev}`, which this service does not declare it \
                         emits. The stream is written by its owner: if the event belongs to \
                         someone else, this is a view, not an aggregate"
                    ));
                }
            }
            // The machine, if declared, has to exist and speak of the same
            // events: two vocabularies for the same concept drift apart at
            // the first change.
            if let Some(mac) = &ag.machine {
                match m.machine.get(mac) {
                    None => errors.push(format!(
                        "{svc}.{nombre}: governed by `[machine.{mac}]`, which does not exist"
                    )),
                    Some(maq) => {
                        for ev in &ag.events {
                            if !maq.transitions.values().any(|t| t.emits.as_ref() == Some(ev)) {
                                errors.push(format!(
                                    "{svc}.{nombre}: `{ev}` belongs to the aggregate and no \
                                     transition of `{mac}` emits it. The generated `fold` \
                                     would not know which state to take it to"
                                ));
                            }
                        }
                    }
                }
            }

            // The aggregate's events go out on the bus, and the stream is already
            // durable: publishing inline after recording leaves a window where
            // the event is in the stream and nobody received it. And publishing
            // BEFORE recording is worse. The handoff has to be durable and in
            // the same transaction as the append — that is the outbox.
            if !ag.events.is_empty() && !m.patterns.outbox {
                errors.push(format!(
                    "{svc}.{nombre}: an aggregate whose events get published needs \
                     `[patterns] outbox = true`. The stream is already durable, so publishing \
                     inline leaves a window where the event is recorded and nobody received \
                     it, and publishing before recording leaves the opposite. The handoff goes \
                     in the SAME transaction as the append"
                ));
            }

            let tabla = Aggregate::tabla(nombre);
            match esquemas.get(svc).and_then(|t| t.get(&tabla)) {
                None => errors.push(format!(
                    "{svc}.{nombre}: the `{tabla}` table is missing. The state IS the stream, and \
                     without the table there is nowhere to put it"
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
                    // Without the UNIQUE, two concurrent writes to the same stream
                    // are both accepted with the same version. Nobody sees an
                    // error and the rebuilt state depends on the read order.
                    let optimista = t.uniques.iter().any(|u| {
                        u.len() == 2
                            && u.iter().any(|c| c == "stream_id")
                            && u.iter().any(|c| c == "version")
                    });
                    if !optimista {
                        errors.push(format!(
                            "{svc}.{nombre}: `{tabla}` has no UNIQUE on (stream_id, version). Two \
                             concurrent writes to the same stream both land with the same \
                             version, with no error at all, and the state that gets rebuilt \
                             depends on what order they are read in"
                        ));
                    }
                }
            }
            // Append-only, and not as a recommendation. A migration that updates
            // or deletes from the stream breaks nothing visible: it leaves a
            // past that did not happen, and everything rebuilt afterwards will
            // be consistent with that lie. There is no `.contract.sql` that
            // enables it, unlike every other table.
            for f in migrations_of(m) {
                let texto = std::fs::read_to_string(&f).unwrap_or_default().to_lowercase();
                let nombre_archivo = f.file_name().unwrap_or_default().to_string_lossy().to_string();
                for verbo in ["update", "delete from", "truncate"] {
                    // look for the verb AND the table in the same statement
                    for sent in texto.split(';') {
                        let limpio: String = sent
                            .lines()
                            .filter(|l| !l.trim_start().starts_with("--"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if limpio.contains(verbo) && limpio.contains(&tabla) {
                            errors.push(format!(
                                "{svc}/{nombre_archivo}: `{}` on `{tabla}`, which is the stream of \
                                 `{nombre}`. A stream is append-only: changing a past event \
                                 leaves a past that did not happen, and everything rebuilt \
                                 afterwards will be consistent with that lie. To correct \
                                 something you add a new event, you do not edit the old one",
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
                        "{svc}.{nombre}: `snapshot_every = {}` with no `{fotos}` table",
                        ag.snapshot_every
                    )),
                    Some(t) => {
                        for (col, para, tipo) in [
                            ("stream_id", "de que instancia es la foto", ""),
                            ("version", "hasta que evento del flujo la cubre", ""),
                            ("estado", "el estado calculado", "json"),
                            (
                                "reglas",
                                "which rules version it was computed with: without this column, \
                                 an old snapshot gets rehydrated with new rules and gives a \
                                 state that no longer matches replaying the stream, with no \
                                 error at all",
                                "",
                            ),
                        ] {
                            match t.col(col) {
                                None => errors.push(format!(
                                    "{svc}.{nombre}: `{fotos}` has no `{col}` column: that is where {para} goes"
                                )),
                                Some(c) if !tipo.is_empty() && !c.ty.to_lowercase().contains(tipo) => {
                                    errors.push(format!(
                                        "{svc}.{nombre}: `{fotos}.{col}` is `{}` and has to be \
                                         {tipo}",
                                        c.ty
                                    ))
                                }
                                _ => {}
                            }
                        }
                        // One snapshot per event is not a cache, it is a second
                        // copy of the stream with twice the writes.
                        if ag.snapshot_every == 1 {
                            warnings.push(format!(
                                "{svc}.{nombre}: `snapshot_every = 1` stores one snapshot per event: that \
                                 is not a cache, it is a second copy of the stream with twice \
                                 the writes"
                            ));
                        }
                    }
                }
            }
        }

        // Read models.
        for (nombre, vi) in &m.view {
            if vi.on.is_empty() {
                errors.push(format!(
                    "{svc}.{nombre}: a view with no events is built from nothing"
                ));
            }
            for ev in &vi.on {
                // It can consume its own events or another service's; what it
                // cannot do is consume one nobody emits.
                if !emitters.contains_key(ev.as_str()) {
                    errors.push(format!(
                        "{svc}.{nombre}: is built from `{ev}`, which nobody emits"
                    ));
                }
                // And if it belongs to another service it has to be declared in
                // `[consumes]`: `axon infra` builds the delivery from there.
                let propio = m.emits.contains_key(ev.as_str());
                if !propio && !m.consumes.contains_key(ev.as_str()) {
                    errors.push(format!(
                        "{svc}.{nombre}: uses `{ev}`, from another service, without declaring it in \
                         `[consumes]`. The subscription comes from there, and without it the \
                         view gets generated and never receives anything"
                    ));
                }
            }
            let tablas = esquemas.get(svc);
            let tabla = vi.tabla(nombre);
            if tablas.and_then(|t| t.get(&tabla)).is_none() {
                errors.push(format!(
                    "{svc}.{nombre}: the `{tabla}` table, where the view lives, is missing"
                ));
            }
            let cp = View::checkpoint(nombre);
            match tablas.and_then(|t| t.get(&cp)) {
                None => errors.push(format!(
                    "{svc}.{nombre}: the `{cp}` table is missing. With nowhere to record how far it \
                     got, a restart either reprocesses from the beginning or skips what it did \
                     not get to apply; both give a wrong view and neither raises an error"
                )),
                Some(t) => {
                    for (col, para) in [
                        ("vista", "cual de las vistas es"),
                        ("stream_id", "de que flujo"),
                        ("posicion", "hasta que version de ESE flujo llego"),
                    ] {
                        if !t.tiene(col) {
                            errors.push(format!(
                                "{svc}.{nombre}: `{cp}` has no `{col}` column: that is where {para} goes"
                            ));
                        }
                    }
                    // Without `stream_id` in the key, one stream overwrites
                    // another's position. An event's version is its position
                    // inside ITS stream: a single number for the whole view
                    // seems to work while there is one stream, and stops
                    // identifying anything as soon as there are two.
                    let por_flujo = t.uniques.iter().any(|u| {
                        u.iter().any(|c| c == "stream_id") && u.iter().any(|c| c == "vista")
                    });
                    if !por_flujo && t.tiene("stream_id") {
                        errors.push(format!(
                            "{svc}.{nombre}: `{cp}` has no key on (vista, stream_id). One stream would \
                             overwrite another's position, and the view would skip events or \
                             reprocess them without anything warning about it"
                        ));
                    }
                }
            }
            // If the view can be rebuilt there is a shadow, and a shadow with a
            // column missing makes the swap leave an incomplete view. That
            // would get discovered on rebuild day, which is the worst day.
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
                        "{svc}.{nombre}: it can be rebuilt and `{sombra}` is missing. Rebuilding in \
                         place leaves the view incomplete while it runs, and it keeps being \
                         read: whoever asks gets fewer rows than there are, with no error"
                    )),
                    (Some(viva), Some(som)) => {
                        for c in &viva.cols {
                            match som.col(&c.name) {
                                None => errors.push(format!(
                                    "{svc}.{nombre}: `{sombra}` has no `{}` column, which `{tabla}` \
                                     does. The swap would leave a view without that data, and \
                                     only then would it show",
                                    c.name
                                )),
                                Some(o) if o.ty != c.ty => errors.push(format!(
                                    "{svc}.{nombre}: `{sombra}.{}` is `{}` and in `{tabla}` it is \
                                     `{}`. On the swap, the view changes type without anything \
                                     saying so",
                                    c.name, o.ty, c.ty
                                )),
                                _ => {}
                            }
                        }
                        for c in &som.cols {
                            if !viva.tiene(&c.name) {
                                warnings.push(format!(
                                    "{svc}.{nombre}: `{sombra}.{}` is not in `{tabla}`. It is \
                                     spare until the next swap, and after that the view is the \
                                     one that has it",
                                    c.name
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }

            // A view is eventual by construction: it gets filled AFTER the event
            // happened. Promising CP on top of it is the same contradiction as
            // reading from a replica and promising CP.
            if !m.cap.eventual() {
                errors.push(format!(
                    "{svc}.{nombre}: a read model with `consistency = \"strong\"`. The view gets \
                     filled after the event happened: what it serves is stale by definition"
                ));
            }
            match (vi.max_staleness_ms, m.cap.max_staleness_ms) {
                (Some(v), Some(tope)) if v > tope => errors.push(format!(
                    "{svc}.{nombre}: the view allows {v}ms of lag and the service declared a limit \
                     of {tope}ms. The service cannot honour what it promised while serving \
                     from a view older than its own budget"
                )),
                (None, _) => warnings.push(format!(
                    "{svc}.{nombre}: no `max_staleness_ms`. With no lag budget, nobody can say \
                     whether the view it served was acceptably stale"
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
