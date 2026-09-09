//! Drift: what turns the manifest into something more than documentation.
use crate::manifest::*;
use indexmap::IndexMap;

/// Governance: the team's rules, versioned alongside the code.
/// Without `axon.policy.toml` these defaults apply.
#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct Policy {
    pub require_owner: bool,
    pub require_tier: bool,
    pub allowed_event_prefixes: Vec<String>,
    pub max_deps_per_service: usize,
    /// The repo layout belongs to the team, not to axon. Without this, `axon ci`
    /// would have to guess it, and guessing is what made it useless.
    pub ci: Ci,
}

/// `{service}` is replaced with the service's name.
#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct Ci {
    pub manifests_dir: String,
    pub service_dir: String,
    pub test_cmd: String,
    pub contracts_path: String,
    /// Absent means "the forge's default", which differs between GitHub and
    /// GitLab: the expression syntax is not portable.
    pub image: Option<String>,
}

impl Default for Ci {
    fn default() -> Self {
        Self {
            manifests_dir: "manifests".into(),
            service_dir: "services/{service}".into(),
            test_cmd: "make -C services/{service} test".into(),
            contracts_path: "services/{service}/src/contracts.ts".into(),
            image: None,
        }
    }
}

impl Ci {
    pub fn path(&self, field: &str, service: &str) -> String {
        field.replace("{service}", service)
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            require_owner: true,
            require_tier: true,
            allowed_event_prefixes: vec![],
            max_deps_per_service: 7, // synchronous coupling: past this, it is a distributed monolith
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

/// A short, deliberately conservative heuristic: only shapes that cannot be
/// the name of an environment variable.
fn parece_secreto(v: &str) -> bool {
    let t = v.trim();
    t.starts_with("sk_")
        || t.starts_with("AKIA")
        || t.starts_with("ghp_")
        || t.starts_with("-----BEGIN")
        || (t.len() > 32 && t.chars().any(|c| c.is_lowercase()) && t.contains(['/', '+', '=']))
}

/// A placeholder is not a value. Without this, `axon import` would produce
/// manifests that pass verify with nobody having reviewed them.
fn pendiente(v: &Option<String>) -> bool {
    match v {
        None => true,
        Some(s) => {
            let s = s.trim().to_uppercase();
            s.is_empty() || s == "TODO" || s == "FIXME" || s == "TBD"
        }
    }
}

/// Whether a TOML value fits a declared type. The manifest's vocabulary is
/// small on purpose, and a catalog entry typed wrong is a row that fails to
/// insert the day somebody applies it, not the day somebody writes it.
fn fits(v: &toml::Value, kind: &str) -> bool {
    match kind {
        "int" => v.is_integer(),
        "float" => v.is_float() || v.is_integer(),
        "bool" => v.is_bool(),
        // string, uuid, timestamp: all text on the wire and in the row
        _ => v.is_str(),
    }
}

fn kind_of(v: &toml::Value) -> &'static str {
    match v {
        toml::Value::Integer(_) => "int",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "bool",
        toml::Value::String(_) => "string",
        _ => "something else",
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
            None => {}
            Some(r) if RUNTIMES.contains(&r) => {}
            Some(other) => errors.push(format!(
                "{}: `runtime = \"{other}\"` does not exist; today there is {}. Another \
                 execution model gets added with an `axon-infra-*` plugin",
                m.service,
                RUNTIMES.join(" and ")
            )),
        }
        // A job runs and ends. Everything that follows is a consequence of
        // that, and each one of these was something that used to apply with no
        // error and leave infrastructure nobody would ever reach.
        if m.infra.is_job() {
            let served: Vec<&String> = m
                .methods
                .iter()
                .filter(|(_, me)| me.http.is_some())
                .map(|(n, _)| n)
                .collect();
            if !served.is_empty() {
                errors.push(format!(
                    "{}: `runtime = \"job\"` and it declares routes ({}). A job is not a \
                     process listening on a port: the routes would be behind an edge that \
                     reaches nothing",
                    m.service,
                    served
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if m.infra.min_instances.is_some() || m.infra.max_instances.is_some() {
                errors.push(format!(
                    "{}: a job has no instances to scale; how many run at a time is the \
                     scheduler's, and `min_instances` here would be an autoscaler over \
                     something that is not up",
                    m.service
                ));
            }
            // Consuming events from something that only runs at nine in the
            // morning is not wrong, but it is a decision: the lag between the
            // event and the reaction is the schedule.
            if !m.consumes.is_empty() {
                warnings.push(format!(
                    "{}: a job that consumes {} events. Between the event and the reaction \
                     there is a whole schedule, and the broker has to retain them meanwhile",
                    m.service,
                    m.consumes.len()
                ));
            }
        } else if m.infra.schedule.is_some() {
            errors.push(format!(
                "{}: it declares `schedule` and is not a `job`. A container that is up does \
                 not get scheduled: what runs on a schedule is something that ends",
                m.service
            ));
        }
        if let Some(sched) = &m.infra.schedule {
            // Five fields, which is what every scheduler of the three providers
            // speaks. Validating more than that would be reimplementing cron;
            // validating less lets `@daily` through, which two of them reject.
            if sched.split_whitespace().count() != 5 {
                errors.push(format!(
                    "{}: `schedule = \"{sched}\"` is not five cron fields. The three \
                     providers speak that and not `@daily`",
                    m.service
                ));
            }
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
                    if is_pii(pii, field) {
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
            if !cols.has(tenant) {
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
        if !WAREHOUSES.contains(&m.analytics.warehouse.as_str()) {
            errors.push(format!(
                "{}: `[analytics] warehouse = \"{}\"` has no dialect. Available: {}",
                m.service,
                m.analytics.warehouse,
                WAREHOUSES.join(", ")
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
    let ahora = today();
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        for (name, f) in &m.flags {
            if f.owner.is_none() {
                errors.push(format!(
                    "{svc}.{name}: flag with no `owner`. Whoever turned it on is who turns it off"
                ));
            }
            // A codebase with two hundred old flags does not have two hundred
            // features: it has two hundred branches nobody tests.
            match (&f.expires, f.kill_switch) {
                (None, false) => errors.push(format!(
                    "{svc}.{name}: flag with no `expires`. A flag with no death date does not die; \
                     if it really is permanent, declare it `kill_switch = true`"
                )),
                (Some(_), true) => warnings.push(format!(
                    "{svc}.{name}: `kill_switch` with `expires`; an emergency switch lives as long as \
                     the thing it turns off does"
                )),
                (Some(e), false) => match date(e) {
                    None => errors.push(format!(
                        "{svc}.{name}: `expires = \"{e}\"` is not in YYYY-MM-DD form"
                    )),
                    Some(f) if f < ahora => errors.push(format!(
                        "{svc}.{name}: expired on {e}. Either the dead branch gets cleaned up or the date \
                         gets renewed as an explicit decision: leaving it expired is neither"
                    )),
                    _ => {}
                },
                _ => {}
            }

            // A per-request rollout makes the SAME entity take one path on one
            // call and the other on the next. With state involved, that leaves
            // data half-migrated.
            //
            // The order of these checks matters: a kill switch with a rollout is
            // an error of its own, not a case of the missing sticky field.
            match f.rollout {
                Some(_) if f.kill_switch => errors.push(format!(
                    "{svc}.{name}: `kill_switch` with `rollout`. An emergency switch turns everything \
                     off or it is worth nothing"
                )),
                Some(p) if p > 100 => errors.push(format!(
                    "{svc}.{name}: `rollout = {p}` is not a percentage"
                )),
                Some(p) if p > 0 && p < 100 && f.sticky_by.is_none() => errors.push(format!(
                    "{svc}.{name}: rollout at {p}% with no `sticky_by`. Evaluated per request, the same \
                     entity takes one path and then the other, and ends up half-migrated"
                )),
                _ => {}
            }
            // A default variant that does not exist makes evaluation always
            // fall back to the value in the code, and the flag quietly stops
            // doing anything: it looks like "the rollout does nothing".
            let variantes = f.all_variants();
            let defecto = f.default_variant();
            if !variantes.contains_key(&defecto) {
                errors.push(format!(
                    "{svc}.{name}: `default_variant = \"{defecto}\"` is not in `variants` ({}). \
                     Evaluation would always fall back to the value in the code",
                    variantes.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            // Mixing types across variants breaks evaluation: OpenFeature
            // resolves one type per flag, not one per variant.
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
                    "{svc}.{name}: the variants mix types ({}). OpenFeature resolves one type per \
                     flag, not one per variant",
                    tipos.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
            if f.default && !f.kill_switch {
                warnings.push(format!(
                    "{svc}.{name}: `default = true` on a flag that is not a kill switch. A new flag on \
                     by default is not a gradual rollout: it is a deploy"
                ));
            }
            // The field it is pinned by has to exist in some contract, or the
            // decision is pinned by data the service never receives.
            if let Some(field) = &f.sticky_by {
                let conocido = m.infra.tenant_column.as_deref() == Some(field.as_str())
                    || m.methods.values().any(|me| me.input.contains_key(field))
                    || m.emits.values().any(|fs| fs.contains_key(field))
                    || ms.iter().any(|o| {
                        m.consumes
                            .keys()
                            .any(|ev| o.emits.get(ev).is_some_and(|fs| fs.contains_key(field)))
                    });
                if !conocido {
                    errors.push(format!(
                        "{svc}.{name}: is pinned by `{field}`, which appears in no contract and is not \
                         the tenant column; the service never receives it"
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
        if !pl.active() {
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
            for (name, me) in m.methods.iter() {
                if me.input.keys().any(|k| normalize(k) == normalize(col)) {
                    continue;
                }
                errors.push(format!(
                    "{svc}.{name}: does not receive `{col}` and the database is sharded by \
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
            // the pooler opens THIS many to EACH engine, and the nodes and the
            // replicas are different engines, each with its own limit
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

    // ---- the cache ----
    //
    // A cache is not another storage engine: it is a derived copy, and the only
    // hard part is knowing when it stopped being true. Every rule here exists
    // because of a failure with NO symptom: the wrong answer, served fast,
    // with every dashboard green.
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        let c = &m.cache;
        if let Some(engine) = &c.engine {
            if !CACHE_ENGINES.contains(&engine.as_str()) {
                errors.push(format!(
                    "{svc}: `[cache] engine = \"{engine}\"` is not supported. Native engines: \
                     {}",
                    CACHE_ENGINES.join(", ")
                ));
            }
        }
        if c.entries.is_empty() {
            continue;
        }
        if !c.active() {
            errors.push(format!(
                "{svc}: declares {} cached answer(s) and no `[cache] engine`. The entries \
                 describe a cache nothing brings up, and the code that reads them would go \
                 to the database every time while the manifest says otherwise",
                c.entries.len()
            ));
            continue;
        }
        // A cache is eventual by construction: between the write and the
        // invalidation there is a window where the old answer is served. A
        // service that promised `strong` and caches is not serving what it
        // promised, and nothing in production says so.
        if !m.cap.eventual() {
            errors.push(format!(
                "{svc}: `consistency = \"strong\"` and a cache. A cache is eventual by \
                 construction —between the change and the invalidation the old answer is \
                 served— so either the promise drops to `eventual` with a \
                 `max_staleness_ms`, or the cache goes"
            ));
        }
        for (name, e) in &c.entries {
            let Some(method) = m.methods.get(&e.of) else {
                errors.push(format!(
                    "{svc}: `[cache.{name}] of = \"{}\"` names no declared method",
                    e.of
                ));
                continue;
            };
            // A key field the method does not receive is a key two different
            // requests share: one customer's answer served to another.
            for k in &e.key {
                if !method.input.contains_key(k) {
                    errors.push(format!(
                        "{svc}: `[cache.{name}] key` names `{k}`, which is not in the `in` of \
                         `{}`. A key built from something the method does not receive is a key \
                         two different requests can share",
                        e.of
                    ));
                }
            }
            if e.key.is_empty() {
                errors.push(format!(
                    "{svc}: `[cache.{name}]` has no `key`. One entry for every call of `{}` is \
                     one answer served to everybody",
                    e.of
                ));
            }
            // The tenant in the key, for the same reason it is in the route and
            // in the RLS: without it the cache is a hole through both.
            if let Some(tenant) = &m.infra.tenant_column {
                let carries = e.key.iter().any(|k| normalize(k) == normalize(tenant));
                if !carries
                    && method
                        .input
                        .keys()
                        .any(|k| normalize(k) == normalize(tenant))
                {
                    errors.push(format!(
                        "{svc}: `[cache.{name}] key` does not carry `{tenant}`, and the service \
                         is multi-tenant. The first tenant to ask warms the entry and the next \
                         one is served their data, as a hit: the RLS and the router never see \
                         the second query"
                    ));
                }
            }
            // Nothing makes it stale: it is not a cache, it is a copy that
            // ages forever.
            if e.ttl_ms.is_none() && e.invalidated_by.is_empty() {
                errors.push(format!(
                    "{svc}: `[cache.{name}]` has neither `ttl_ms` nor `invalidated_by`. \
                     Nothing makes it stale, so the first answer is served forever"
                ));
            }
            // The TTL is what keeps the promise `[cap]` made. Longer than the
            // budget and the number in the manifest is a number nobody meets.
            // `stale_ms` SPENDS staleness too: what is served during the
            // refresh is as old as the reader sees it.
            let stale = e.stale_ms.unwrap_or(0);
            if let (Some(ttl), Some(budget)) = (e.ttl_ms, m.cap.max_staleness_ms) {
                if ttl + stale > budget {
                    errors.push(format!(
                        "{svc}: `[cache.{name}]` serves an answer up to {} ms old (`ttl_ms` \
                         {ttl}{}) against a declared `max_staleness_ms = {budget}`. The budget \
                         is the promise and these are what keep it",
                        ttl + stale,
                        match e.stale_ms {
                            Some(s) => format!(" + `stale_ms` {s}"),
                            None => String::new(),
                        }
                    ));
                }
            }
            if e.stale_ms.is_some() && e.ttl_ms.is_none() {
                errors.push(format!(
                    "{svc}: `[cache.{name}] stale_ms` with no `ttl_ms`. Nothing ever expires, \
                     so nothing is ever served stale-while-revalidating: the field promises a \
                     behaviour that cannot happen"
                ));
            }
            // The switch. A cache behind a flag can be turned off the day its
            // invalidation turns out to be wrong, without a deploy — and a
            // `[rules.*]` over a metric can be the one that turns it off.
            if let Some(flag) = &e.enabled_by {
                match m.flags.get(flag) {
                    None => errors.push(format!(
                        "{svc}: `[cache.{name}] enabled_by = \"{flag}\"` names no declared flag"
                    )),
                    Some(f) if f.kind() != "boolean" => errors.push(format!(
                        "{svc}: `[cache.{name}] enabled_by = \"{flag}\"` is a {} flag, and a \
                         cache is on or off",
                        f.kind()
                    )),
                    Some(f) => {
                        if let Some(sticky) = &f.sticky_by {
                            if !method
                                .input
                                .keys()
                                .any(|k| normalize(k) == normalize(sticky))
                            {
                                errors.push(format!(
                                    "{svc}: `[cache.{name}]` is gated by `{flag}`, pinned by \
                                     `{sticky}`, and `{}` does not receive it. The decision \
                                     would be taken per request, so the same entity would hit \
                                     the cache on one call and not on the next",
                                    e.of
                                ));
                            }
                        }
                    }
                }
            }
            let strategy = e.strategy.as_deref().unwrap_or("invalidate");
            if !CACHE_STRATEGIES.contains(&strategy) {
                errors.push(format!(
                    "{svc}: `[cache.{name}] strategy = \"{strategy}\"` is not one of {}",
                    CACHE_STRATEGIES.join(", ")
                ));
            }
            // `refresh` rewrites the entry from the event instead of dropping
            // it, which is only possible if the event carries the whole
            // answer. That is checkable, so it is checked: otherwise the cache
            // fills with holes and every hole is served as if it were data.
            if strategy == "refresh" {
                if e.invalidated_by.is_empty() {
                    errors.push(format!(
                        "{svc}: `[cache.{name}] strategy = \"refresh\"` with no \
                         `invalidated_by`. There is no event to rewrite it from"
                    ));
                }
                for ev in &e.invalidated_by {
                    let Some(fields) = ms.iter().find_map(|o| o.emits.get(ev)) else {
                        continue;
                    };
                    let missing: Vec<&String> = method
                        .output
                        .keys()
                        .filter(|f| !fields.keys().any(|g| normalize(g) == normalize(f)))
                        .collect();
                    if let Some(f) = missing.first() {
                        errors.push(format!(
                            "{svc}: `[cache.{name}] strategy = \"refresh\"` rewrites the \
                             answer of `{}` from `{ev}`, and that event does not carry `{f}`. \
                             The entry would be rewritten with a hole, and a hole is served \
                             exactly like data. Either the event carries it or the strategy is \
                             `invalidate`",
                            e.of
                        ));
                    }
                }
            }
            // Personal data with no bound is personal data kept forever in a
            // place nobody lists when they answer a deletion request.
            let sensitive: Vec<&String> = method
                .output
                .keys()
                .filter(|f| m.pii.iter().any(|p| normalize(p) == normalize(f)))
                .collect();
            if let Some(field) = sensitive.first() {
                match e.ttl_ms {
                    None => errors.push(format!(
                        "{svc}: `[cache.{name}]` caches `{}`, whose answer carries `{field}` \
                         —declared `pii`— with no `ttl_ms`. Personal data with no bound is \
                         personal data kept forever somewhere nobody lists when a deletion \
                         request arrives",
                        e.of
                    )),
                    Some(_) => warnings.push(format!(
                        "{svc}: `[cache.{name}]` caches `{field}`, which is declared `pii`. It \
                         is bounded by its `ttl_ms`, and it is still a second copy of personal \
                         data outside the database",
                    )),
                }
            }
            for ev in &e.invalidated_by {
                let emitted = ms.iter().any(|o| o.emits.contains_key(ev));
                if !emitted {
                    errors.push(format!(
                        "{svc}: `[cache.{name}] invalidated_by` names `{ev}`, which nobody \
                         emits"
                    ));
                    continue;
                }
                // The invalidation has to be able to BUILD the key, and the
                // key is built from the event. A field the event does not
                // carry makes a key with a hole in it: the `del` runs, deletes
                // nothing, and the stale answer is served until the TTL —
                // which is the failure this whole block exists to prevent,
                // reintroduced by the generated code itself.
                if let Some(fields) = ms.iter().find_map(|o| o.emits.get(ev)) {
                    for k in &e.key {
                        if !fields.keys().any(|g| normalize(g) == normalize(k)) {
                            errors.push(format!(
                                "{svc}: `[cache.{name}]` is keyed by `{k}` and `{ev}` does not \
                                 carry it, so the invalidation cannot build the key: it would \
                                 delete nothing and the stale answer would be served until the \
                                 `ttl_ms`. Either the event carries `{k}` or it is not what \
                                 makes this entry stale"
                            ));
                        }
                    }
                }
                // An invalidation this service cannot hear is worse than none:
                // it reads as handled and never runs.
                let visible = m.emits.contains_key(ev) || m.consumes.contains_key(ev);
                if !visible {
                    errors.push(format!(
                        "{svc}: `[cache.{name}]` says `{ev}` makes it stale and this service \
                         neither emits nor consumes it, so it cannot hear it. Declare it in \
                         `[consumes]` or the invalidation is one nobody runs"
                    ));
                }
            }
            // And the compensation. This is the distributed-transaction case:
            // a step succeeds, the cache is invalidated and warmed again with
            // the new value, the saga fails and compensates — and the cache
            // keeps serving the value of the attempt that was rolled back.
            // `compensates` is declared, so this is derivable and not a guess.
            for mac in m.machine.values() {
                for t in mac.transitions.values() {
                    let Some(emitted) = &t.emits else { continue };
                    if !e.invalidated_by.contains(emitted) {
                        continue;
                    }
                    for (undo_name, undo) in &mac.transitions {
                        let compensates_this = undo.compensates.as_ref().is_some_and(|c| {
                            mac.transitions
                                .get(c)
                                .is_some_and(|x| x.emits.as_ref() == Some(emitted))
                        });
                        if !compensates_this {
                            continue;
                        }
                        match &undo.emits {
                            Some(back) if !e.invalidated_by.contains(back) => {
                                errors.push(format!(
                                    "{svc}: `[cache.{name}]` is invalidated by `{emitted}` and \
                                     not by `{back}`, which is what `{undo_name}` emits when it \
                                     compensates it. After a rollback the cache keeps serving \
                                     the value of the attempt that was undone"
                                ));
                            }
                            None => warnings.push(format!(
                                "{svc}: `{undo_name}` compensates what invalidates \
                                 `[cache.{name}]` and emits nothing, so the compensation \
                                 cannot invalidate anything. The cache keeps the value of the \
                                 attempt that was undone until its `ttl_ms`"
                            )),
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    // ---- catalogs ----
    //
    // The list nobody thinks is worth declaring: currencies, statuses,
    // reasons. It ends up written three times —an enum, a CHECK, a dropdown—
    // and the day somebody adds a value two of the three do not hear about it.
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        for (name, cat) in &m.catalog {
            if !cat.fields.contains_key(&cat.key) {
                errors.push(format!(
                    "{svc}: `[catalog.{name}] key = \"{}\"` is not one of its `fields`",
                    cat.key
                ));
                continue;
            }
            if cat.entries.is_empty() {
                warnings.push(format!(
                    "{svc}: `[catalog.{name}]` has no entries. The table and the type come out \
                     empty, and an empty union type makes every value invalid"
                ));
            }
            let mut seen: Vec<String> = Vec::new();
            for (i, entry) in cat.entries.iter().enumerate() {
                // Every entry carries every field: a hole in a catalog is a
                // NULL where the code declared a value, and the union type
                // would say otherwise.
                for (field, kind) in &cat.fields {
                    match entry.get(field) {
                        None => errors.push(format!(
                            "{svc}: `[catalog.{name}]` entry {i} has no `{field}`. A catalog \
                             with holes is a NULL where the generated type promises a value"
                        )),
                        Some(v) if !fits(v, kind) => errors.push(format!(
                            "{svc}: `[catalog.{name}]` entry {i}: `{field}` is `{}` and the \
                             field is declared `{kind}`",
                            kind_of(v)
                        )),
                        _ => {}
                    }
                }
                // An unknown field is a value somebody meant to declare and
                // that nothing will store: it is not in the table.
                for field in entry.keys() {
                    if !cat.fields.contains_key(field) {
                        errors.push(format!(
                            "{svc}: `[catalog.{name}]` entry {i} carries `{field}`, which is \
                             not one of its `fields`: it would be dropped in silence"
                        ));
                    }
                }
                if let Some(k) = entry.get(&cat.key) {
                    let k = k
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| k.to_string());
                    if seen.contains(&k) {
                        errors.push(format!(
                            "{svc}: `[catalog.{name}]` repeats the key `{k}`. The seed is an \
                             upsert, so the second one silently wins"
                        ));
                    }
                    seen.push(k);
                }
            }
            // A catalog needs somewhere to live: it is a table.
            if m.infra.state.is_none() {
                errors.push(format!(
                    "{svc}: `[catalog.{name}]` with no `[infra] state`. The list has nowhere to \
                     be seeded, and half of what declaring it buys is that the database knows \
                     it too"
                ));
            }
        }
    }

    // ---- the engine has to exist ----
    for m in ms.iter().filter(|m| !m.external) {
        let Some(motor) = &m.infra.state else {
            continue;
        };
        if !ENGINES.contains(&motor.as_str()) {
            // "none" is the one somebody writes to mean "this service has no
            // database". Saying it needs a plugin sends them to build one for
            // nothing: what they want is to not declare the field.
            if motor == "none" {
                errors.push(format!(
                    "{}: `state = \"none\"` is not an engine. A service with no database of its \
                     own declares no `state` at all, and then no rule about pools, replicas or \
                     backups applies to it",
                    m.service
                ));
            } else {
                errors.push(format!(
                    "{}: `state = \"{motor}\"` is not supported. Native engines: {}. A different \
                     one is served by an `axon-infra-{motor}` plugin, which receives the neutral \
                     plan on stdin; without that, axon would generate Postgres infrastructure \
                     for something that is not Postgres",
                    m.service,
                    ENGINES.join(", ")
                ));
            }
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
            (inf.pool_size, inf.max_connections, m.pooler.active())
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
        // High availability is not a backup: a standby replicates the DROP
        // TABLE within seconds. Two different problems with two different
        // solutions.
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

        // A replica lags. Reading from it and promising strong consistency is
        // the theorem's contradiction, written in two places.
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
        let Some(key) = &m.infra.shard_key else {
            continue;
        };
        let Some(tablas) = esquemas_shard.get(svc) else {
            continue;
        };

        // Isolating by one column and sharding by another makes every query from
        // one tenant touch every node: the sharding stops being worth anything.
        if let Some(inq) = &m.infra.tenant_column {
            if inq != key {
                errors.push(format!(
                    "{svc}: isolates by `{inq}` and shards by `{key}`. Every query from one \
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
            .filter(|(_, t)| t.has(key))
            .map(|(t, _)| t)
            .collect();

        for (t, tb) in tablas {
            if ["outbox", "inbox_seen"].contains(&t.as_str()) || m.infra.tenant_exempt.contains(t) {
                continue;
            }
            if !repartidas.contains(&t) {
                errors.push(format!(
                    "{svc}.{t}: no `{key}` column, so it cannot be sharded. Add it, or \
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
                if !u.iter().any(|c| c == key) {
                    errors.push(format!(
                        "{svc}.{t}: `UNIQUE ({})` does not include `{key}`. Each node \
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
                        "{svc}.{t}.{}: FK to `{fk}`, which does not carry `{key}`. A FK \
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
        if !cap.declared {
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
        if !cap.eventual() && cap.degrades() {
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
        for (name, mac) in &m.machine {
            for (act, t) in &mac.transitions {
                if !cap.eventual() && m.consumes.contains_key(&t.on) {
                    warnings.push(format!(
                        "{svc}.{name}.{act}: `strong` transition triggered by the event \
                         `{}`, which arrives eventually; the state may have changed first",
                        t.on
                    ));
                }
            }
        }
    }

    // A08: integrity failures. A tag is mutable: what gets deployed today is
    // not what was audited yesterday.
    if let Some(img) = &pol.ci.image {
        if img.contains(":latest") || !img.contains('@') {
            warnings.push(format!(
                "[A08] [ci].image `{img}` does not pin a digest; a tag is mutable and the \
                 deploy stops being reproducible"
            ));
        }
    }

    // API patterns: what separates an endpoint from one that survives production
    let mut routes: IndexMap<String, String> = IndexMap::new();
    for m in ms.iter().filter(|m| !m.external) {
        for (name, meth) in &m.methods {
            let Some(http) = &meth.http else { continue };
            if let Some(prev) = routes.insert(http.clone(), m.service.clone()) {
                errors.push(format!(
                    "`{http}` is declared by {prev} and by {}",
                    m.service
                ));
            }
            // With `versioning = "header"` the route carries no version on
            // purpose: it is the caller who pins one, and the same route serves
            // every version.
            match meth.path() {
                Some(p) if !m.api.by_header() && !p.starts_with("/v") => errors.push(format!(
                    "{}.{name}: `{p}` has no version in the path; use /v1/... or declare `[api] versioning = \"header\"`",
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
            // Scopes. `auth = "required"` says the caller is somebody; a scope
            // says the caller is somebody allowed to do THIS, which is a
            // different question and the one that decides whether any valid
            // token can issue a refund.
            let catalogue = &m.api.scopes;
            if meth.auth.as_deref() == Some("public") && !meth.scopes.is_empty() {
                errors.push(format!(
                    "{}.{name}: it is `public` and demands scopes. Nobody presents a token on \
                     a public route: it is either open or it is not",
                    m.service
                ));
            }
            for sc in &meth.scopes {
                if !catalogue.is_empty() && !catalogue.contains(sc) {
                    errors.push(format!(
                        "{}.{name}: `{sc}` is not in `[api] scopes`. A scope with a typo is a \
                         403 in production that nobody sees in a review",
                        m.service
                    ));
                }
            }
            // A mutation behind a token and nothing else: whoever can read can
            // also refund, and that is a decision nobody took on purpose.
            if meth.mutating() && meth.auth.as_deref() == Some("required") && meth.scopes.is_empty()
            {
                warnings.push(format!(
                    "{}.{name}: {http} mutates behind `auth = \"required\"` and no `scopes`. \
                     Any valid token can call it, including one issued to read",
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

    // Rules over a metric. What they propose is a decision that today lives in
    // an alert plus a runbook, and the point of declaring it is the same as
    // everywhere else here: it can be refuted.
    for m in ms.iter().filter(|m| !m.external) {
        for (name, r) in &m.rules {
            match r.mode.as_deref() {
                None | Some("propose") => {}
                // It is not blocked, and it is not quiet either. A rule that
                // moves a lever on its own is a control loop over production,
                // and whoever reads this output should see it named every time
                // and not only the day somebody wrote it.
                Some("apply") => warnings.push(format!(
                    "{}.{name}: `mode = \"apply\"` moves `{}` on its own. It is a control \
                     loop over production: it still takes `axon rules --apply` to happen, \
                     and the audit trail is the only record that it did",
                    m.service,
                    r.then.flag.as_deref().unwrap_or("something")
                )),
                Some(other) => errors.push(format!(
                    "{}.{name}: `mode = \"{other}\"` does not exist; it is \"propose\" or \
                     \"apply\"",
                    m.service
                )),
            }
            // Applying anything other than a flag would mean axon emitting an
            // event or calling a method on its own, which is a different tool.
            if r.mode.as_deref() == Some("apply") && r.then.flag.is_none() {
                errors.push(format!(
                    "{}.{name}: `mode = \"apply\"` only moves a flag. Emitting an event or \
                     calling a method on its own is not something axon does: those stay in \
                     `propose`",
                    m.service
                ));
            }
            let metric = m.metrics.get(&r.metric);
            match metric {
                None => errors.push(format!(
                    "{}.{name}: watches `{}`, which is not a metric of the service. A rule \
                     over a metric nobody declares never fires, and never firing reads \
                     exactly like everything being fine",
                    m.service, r.metric
                )),
                Some(mt) => {
                    // A metric grouped by something is one series per group.
                    // Comparing without saying which group is comparing one
                    // group's number against another's, and that answer is not
                    // wrong in a way anybody notices.
                    let missing: Vec<&String> = mt
                        .by
                        .iter()
                        .filter(|dim| {
                            !r.segment
                                .keys()
                                .any(|k| crate::bi::snake(k) == crate::bi::snake(dim))
                        })
                        .collect();
                    if !missing.is_empty() {
                        errors.push(format!(
                            "{}.{name}: `{}` is grouped by {}, and the rule does not pin {}. \
                             It is one series per group: unpinned, it compares one group's \
                             number against another's",
                            m.service,
                            r.metric,
                            mt.by.join(", "),
                            missing
                                .iter()
                                .map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    for k in r.segment.keys() {
                        if !mt
                            .by
                            .iter()
                            .any(|dim| crate::bi::snake(dim) == crate::bi::snake(k))
                        {
                            errors.push(format!(
                                "{}.{name}: `where.{k}` is not a dimension of `{}`. The \
                                 segment has to be something the metric groups by, or there \
                                 is nothing to filter",
                                m.service, r.metric
                            ));
                        }
                    }
                }
            }
            if !COMPARISONS.contains(&r.comparison()) {
                errors.push(format!(
                    "{}.{name}: `compare = \"{}\"` does not exist; use {}",
                    m.service,
                    r.comparison(),
                    COMPARISONS.join(", ")
                ));
            }
            // Exactly one threshold: with both, whichever fired would depend on
            // the order they were read in; with none there is no condition.
            match (r.below, r.above) {
                (None, None) => errors.push(format!(
                    "{}.{name}: no `below` nor `above`; there is no condition to hold",
                    m.service
                )),
                (Some(_), Some(_)) => errors.push(format!(
                    "{}.{name}: `below` and `above` at the same time",
                    m.service
                )),
                _ => {}
            }
            if r.comparison() == "absolute" && r.value.is_none() {
                errors.push(format!(
                    "{}.{name}: `compare = \"absolute\"` with no `value` to compare against",
                    m.service
                ));
            }
            if r.comparison() != "absolute" && r.value.is_some() {
                errors.push(format!(
                    "{}.{name}: `value` only means something with `compare = \"absolute\"`",
                    m.service
                ));
            }
            if r.sustained == 0 {
                errors.push(format!(
                    "{}.{name}: `for = 0`; a condition that holds for no window is not a \
                     condition",
                    m.service
                ));
            }
            // Without a cooldown it proposes every window while the condition
            // lasts, and a rule that repeats itself gets ignored, which is the
            // same as not having it.
            match r.cooldown {
                None => errors.push(format!(
                    "{}.{name}: no `cooldown`. It would propose the same thing every window \
                     while the condition lasts, and what repeats gets ignored",
                    m.service
                )),
                Some(0) => errors.push(format!("{}.{name}: `cooldown = 0`", m.service)),
                Some(_) => {}
            }
            // Guards: the other direction, declared. Same checks as the
            // trigger, because a guard nobody can read is worse than no guard:
            // it reads like the rule is being watched.
            for g in &r.guards {
                match m.metrics.get(&g.metric) {
                    None => errors.push(format!(
                        "{}.{name}: the guard watches `{}`, which is not a metric of the \
                         service. A guard over a metric nobody declares never holds, and the \
                         rule would never propose while reading as if it were guarded",
                        m.service, g.metric
                    )),
                    Some(mt) => {
                        let missing: Vec<&String> = mt
                            .by
                            .iter()
                            .filter(|dim| {
                                !g.segment
                                    .keys()
                                    .any(|k| crate::bi::snake(k) == crate::bi::snake(dim))
                            })
                            .collect();
                        if !missing.is_empty() {
                            errors.push(format!(
                                "{}.{name}: the guard over `{}` does not pin {}",
                                m.service,
                                g.metric,
                                missing
                                    .iter()
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        }
                        for k in g.segment.keys() {
                            if !mt
                                .by
                                .iter()
                                .any(|dim| crate::bi::snake(dim) == crate::bi::snake(k))
                            {
                                errors.push(format!(
                                    "{}.{name}: `guard.where.{k}` is not a dimension of `{}`",
                                    m.service, g.metric
                                ));
                            }
                        }
                    }
                }
                if !COMPARISONS.contains(&g.comparison()) {
                    errors.push(format!(
                        "{}.{name}: the guard's `compare = \"{}\"` does not exist",
                        m.service,
                        g.comparison()
                    ));
                }
                match (g.below, g.above) {
                    (None, None) => errors.push(format!(
                        "{}.{name}: a guard with no `below` nor `above` holds always, which is \
                         the same as not being there",
                        m.service
                    )),
                    (Some(_), Some(_)) => errors.push(format!(
                        "{}.{name}: the guard has `below` and `above` at the same time",
                        m.service
                    )),
                    _ => {}
                }
                // The same condition twice is not a guard: it is the trigger
                // written again, and it would hold exactly when the trigger does.
                if g.metric == r.metric
                    && g.segment == r.segment
                    && g.below.is_some() == r.below.is_some()
                {
                    errors.push(format!(
                        "{}.{name}: the guard is the trigger written again —same metric, same \
                         segment, same direction—, so it holds exactly when the trigger does \
                         and guards nothing",
                        m.service
                    ));
                }
            }
            // Goodhart, named: a lever judged by one number, with nothing
            // watching what it moves the other way.
            if r.guards.is_empty() && r.actions() == 1 {
                warnings.push(format!(
                    "{}.{name}: it moves a lever off one metric and declares no `guard`. That \
                     is Goodhart's law with a cron: the lever moves the number it is judged \
                     by, and nobody is watching what it moves in the other direction",
                    m.service
                ));
            }
            if r.actions() != 1 {
                errors.push(format!(
                    "{}.{name}: it proposes {} things; declare exactly one of `flag`, \
                     `emits` or `calls`",
                    m.service,
                    r.actions()
                ));
            }
            if let Some(flag) = &r.then.flag {
                match m.flags.get(flag) {
                    None => errors.push(format!(
                        "{}.{name}: `flag = \"{flag}\"` is not a flag of the service",
                        m.service
                    )),
                    Some(f) => {
                        // An emergency switch is a person's, and a rule that
                        // flips it takes away the one thing it is for.
                        if f.kill_switch {
                            errors.push(format!(
                                "{}.{name}: `{flag}` is a `kill_switch`, and that switch is a \
                                 person's. A rule that flips it takes away the only thing it \
                                 exists for",
                                m.service
                            ));
                        }
                        for (which, v) in
                            [("variant", &r.then.variant), ("restore", &r.then.restore)]
                        {
                            match v {
                                None => errors.push(format!(
                                    "{}.{name}: it proposes `{flag}` with no `{which}`{}",
                                    m.service,
                                    if which == "restore" {
                                        ". What goes up on its own has to be able to come back down on its own"
                                    } else {
                                        ""
                                    }
                                )),
                                Some(v) if !f.variants.contains_key(v) => errors.push(format!(
                                    "{}.{name}: `{which} = \"{v}\"` is not a variant of `{flag}`",
                                    m.service
                                )),
                                _ => {}
                            }
                        }
                        if r.then.variant.is_some() && r.then.variant == r.then.restore {
                            errors.push(format!(
                                "{}.{name}: `variant` and `restore` are the same, so it \
                                 proposes changing nothing",
                                m.service
                            ));
                        }
                    }
                }
            }
            if let Some(ev) = &r.then.emits {
                if !m.emits.contains_key(ev) {
                    errors.push(format!(
                        "{}.{name}: it proposes emitting `{ev}`, which the service does not \
                         declare in `[emits]`",
                        m.service
                    ));
                }
            }
            if let Some(met) = &r.then.calls {
                if !m.methods.contains_key(met) {
                    errors.push(format!(
                        "{}.{name}: it proposes calling `{met}`, which is not a method of the \
                         service",
                        m.service
                    ));
                }
            }
        }
        // Two rules over the same flag fight, and which one won would depend on
        // the order they were read in.
        let mut owner: IndexMap<&str, &str> = IndexMap::new();
        for (name, r) in &m.rules {
            if let Some(flag) = &r.then.flag {
                if let Some(prev) = owner.insert(flag.as_str(), name.as_str()) {
                    errors.push(format!(
                        "{}: `{prev}` and `{name}` both propose over `{flag}`. One flag has \
                         one rule, or which one won would depend on the order",
                        m.service
                    ));
                }
            }
        }
    }

    // Declared consumption: which fields each consumer really reads.
    //
    // This is what a producer cannot answer on its own, and the reason a
    // contract ends up frozen: "somebody might be using it". Declared, the
    // question has an answer — and it cannot drift, because the generated code
    // hands the consumer `Pick<..., uses>` and reading anything else does not
    // compile.
    for m in ms.iter().filter(|m| !m.external) {
        for d in &m.depends {
            let Some(uses) = &d.uses else { continue };
            let tgt = d.target();
            let Some(sig) = ms
                .iter()
                .find(|o| o.service == tgt)
                .and_then(|o| o.methods.get(&d.method))
            else {
                continue;
            };
            for f in uses {
                if !sig.output.contains_key(f) {
                    errors.push(format!(
                        "{} declares it uses `{tgt}.{}.{f}`, which {tgt} does not return. \
                         Either it is a field that was renamed and the caller is reading \
                         `undefined`, or the declaration is wrong",
                        m.service, d.method
                    ));
                }
            }
        }
        for (ev, c) in &m.consumes {
            let Some(fields) = ms.iter().find_map(|o| o.emits.get(ev)) else {
                continue;
            };
            for f in c.uses.iter().flatten() {
                if !fields.contains_key(f) {
                    errors.push(format!(
                        "{} declares it uses `{ev}.{f}`, which the event does not carry",
                        m.service
                    ));
                }
            }
        }
    }
    // And the answer nobody has today: a field NOBODY reads.
    //
    // Only when every consumer declared what it uses. With one that declared
    // nothing there is no answer, and a warning saying "delete it" without an
    // answer is how a field somebody was reading gets deleted.
    for m in ms.iter().filter(|m| !m.external) {
        // The warehouse reads EVERY field of an exported event, so for an
        // exported one nobody-reads-it is false. A rule with a false positive
        // gets silenced wholesale, and this one has to survive to be worth
        // anything. A method's answer does not go to the warehouse, so the
        // exemption stops at the events.
        for (ev, fields) in m.emits.iter().filter(|_| !m.analytics.export) {
            let consumers: Vec<&Manifest> = ms
                .iter()
                .filter(|o| !o.external && o.consumes.contains_key(ev))
                .collect();
            if consumers.is_empty() || consumers.iter().any(|c| c.consumes[ev].uses.is_none()) {
                continue;
            }
            for f in fields.keys() {
                if !consumers
                    .iter()
                    .any(|c| c.consumes[ev].uses.iter().flatten().any(|u| u == f))
                {
                    warnings.push(format!(
                        "{ev}.{f} is read by nobody: every consumer declared what it uses and \
                         none of them names it. It can be removed in the next version",
                    ));
                }
            }
        }
        for (name, meth) in &m.methods {
            let callers: Vec<&Manifest> = ms
                .iter()
                .filter(|o| !o.external)
                .filter(|o| {
                    o.depends
                        .iter()
                        .any(|d| d.target() == m.service && &d.method == name)
                })
                .collect();
            if callers.is_empty()
                || callers.iter().any(|c| {
                    c.depends
                        .iter()
                        .filter(|d| d.target() == m.service && &d.method == name)
                        .any(|d| d.uses.is_none())
                })
            {
                continue;
            }
            for f in meth.output.keys() {
                if !callers.iter().any(|c| {
                    c.depends
                        .iter()
                        .filter(|d| d.target() == m.service && &d.method == name)
                        .any(|d| d.uses.iter().flatten().any(|u| u == f))
                }) {
                    warnings.push(format!(
                        "{}.{name} returns `{f}` and no caller reads it: every one of them \
                         declared what it uses. It can stop being returned",
                        m.service
                    ));
                }
            }
        }
    }

    // The API's versioning, and its maintenance cycle.
    //
    // A platform decision: `verify` requires every service to declare the same
    // one, for the same reason it requires one warehouse. With two schemes at
    // once a caller has to know which service it is talking to before it can
    // know how to ask for a version, which is the opposite of what versioning
    // is for.
    let internal: Vec<&Manifest> = ms.iter().filter(|m| !m.external).collect();
    if let Some(first) = internal.first() {
        for other in internal.iter().skip(1) {
            if other.api.versioning != first.api.versioning
                || other.api.dates() != first.api.dates()
                || other.api.default != first.api.default
                || other.api.scopes != first.api.scopes
            {
                errors.push(format!(
                    "{} and {} declare different `[api]`. The versioning is one decision for \
                     the whole platform: with two, whoever calls has to know which service \
                     it is talking to before it can know how to ask for a version",
                    first.service, other.service
                ));
                break;
            }
        }
    }
    let api = internal.first().map(|m| &m.api);
    if let Some(api) = api {
        match api.versioning.as_deref() {
            None | Some("path") | Some("header") => {}
            Some(other) => errors.push(format!(
                "[api] `versioning = \"{other}\"` does not exist; use \"path\" or \"header\""
            )),
        }
        if !api.by_header() && !api.versions.is_empty() {
            errors.push(
                "[api] declares dated versions with `versioning` that is not \"header\"; in \
                 the path scheme the version IS the route, and these would be served by nobody"
                    .to_string(),
            );
        }
        if api.by_header() && api.versions.is_empty() {
            errors.push(
                "[api] `versioning = \"header\"` with no `[[api.version]]`; there is nothing \
                 for the caller to pin"
                    .to_string(),
            );
        }
        let mut previous: Option<(i64, i64, i64)> = None;
        let mut seen: IndexMap<String, ()> = IndexMap::new();
        for v in &api.versions {
            let Some(d) = date(&v.date) else {
                errors.push(format!(
                    "[api] version `{}` is not in YYYY-MM-DD form",
                    v.date
                ));
                continue;
            };
            if seen.insert(v.date.clone(), ()).is_some() {
                errors.push(format!("[api] version `{}` is declared twice", v.date));
            }
            // Ordered oldest first: the adapter chain is applied in this order,
            // so a list out of order does not read badly, it adapts backwards.
            if previous.is_some_and(|p| d < p) {
                errors.push(format!(
                    "[api] version `{}` comes after a newer one. The list is the order the \
                     adapters are applied in, so out of order it adapts backwards",
                    v.date
                ));
            }
            previous = Some(d);

            // The maintenance cycle, which is the part that can be refuted.
            match (&v.sunset, v.lts) {
                (None, true) => errors.push(format!(
                    "[api] the LTS `{}` has no `sunset`. \"Long term\" with no date is not a \
                     promise, it is a hope, and it is why a version from years ago is still up",
                    v.date
                )),
                (Some(su), _) => match date(su) {
                    None => errors.push(format!(
                        "[api] `{}`: `sunset = \"{su}\"` is not in YYYY-MM-DD form",
                        v.date
                    )),
                    Some(sd) => {
                        if sd < d {
                            errors.push(format!(
                                "[api] `{}` sunsets on {su}, before it shipped",
                                v.date
                            ));
                        }
                        if sd < ahora {
                            errors.push(format!(
                                "[api] `{}` sunset on {su} and it is still declared. Either the \
                                 version goes or the date gets renewed as a decision somebody makes",
                                v.date
                            ));
                        }
                        // The declared window, applied. Without this the promise
                        // lives in a blog post and dies in a sprint.
                        let window = if v.lts {
                            api.lts_window_days.or(api.support_window_days)
                        } else {
                            api.support_window_days
                        };
                        if let Some(w) = window {
                            let lived = epoch_days(sd) - epoch_days(d);
                            if lived < w {
                                errors.push(format!(
                                    "[api] `{}` is served for {lived} days and the declared \
                                     {}window is {w}",
                                    v.date,
                                    if v.lts { "LTS " } else { "" }
                                ));
                            }
                        }
                    }
                },
                (None, false) => {
                    if api.newest().map(|n| n.date.as_str()) != Some(v.date.as_str()) {
                        warnings.push(format!(
                            "[api] `{}` is not the newest and has no `sunset`; a version with \
                             no death date does not die",
                            v.date
                        ));
                    }
                }
            }
            if let (Some(dep), Some(su)) = (
                v.deprecated.as_deref().and_then(date),
                v.sunset.as_deref().and_then(date),
            ) {
                if su < dep {
                    errors.push(format!(
                        "[api] `{}` sunsets before it is deprecated; there is no window to \
                         migrate in",
                        v.date
                    ));
                }
            }
            // An LTS that dies before the ordinary version that follows it is
            // not long-term at all.
            if v.lts {
                if let Some(mine) = v.sunset.as_deref().and_then(date) {
                    for other in api.versions.iter().filter(|o| !o.lts && o.date > v.date) {
                        if other
                            .sunset
                            .as_deref()
                            .and_then(date)
                            .is_some_and(|o| o > mine)
                        {
                            errors.push(format!(
                                "[api] the LTS `{}` dies before `{}`, which is not LTS. Then it \
                                 is not long-term support, it is just a label",
                                v.date, other.date
                            ));
                        }
                    }
                }
            }
        }
        if let Some(def) = &api.default {
            if api.find(def).is_none() {
                errors.push(format!(
                    "[api] `default = \"{def}\"` is not one of the declared versions"
                ));
            } else if api.newest().map(|n| &n.date) != Some(def) {
                warnings.push(format!(
                    "[api] the default is `{def}` and the newest is `{}`; a new integration \
                     that pins nothing lands on an old version",
                    api.newest().map(|n| n.date.as_str()).unwrap_or_default()
                ));
            }
        }
    }
    // The two schemes cannot be mixed, and a shape from the past needs somebody
    // to translate it: that is what makes one implementation able to serve a
    // version from years ago.
    for m in ms.iter().filter(|m| !m.external) {
        for (name, meth) in &m.methods {
            if m.api.by_header() {
                if let Some(p) = meth.path() {
                    if p.starts_with("/v")
                        && p.get(2..3)
                            .is_some_and(|c| c.chars().all(|c| c.is_ascii_digit()))
                    {
                        errors.push(format!(
                            "{}.{name}: `{p}` versions the route while `[api]` versions by \
                             header; two schemes at once means two answers to the same question",
                            m.service
                        ));
                    }
                }
            } else if !meth.at.is_empty() {
                errors.push(format!(
                    "{}.{name}: declares `at` shapes and `[api] versioning` is not \"header\"; \
                     in the path scheme an old shape is an old route",
                    m.service
                ));
            }
            let mut adapters: IndexMap<&str, &str> = IndexMap::new();
            for (ver, shape) in &meth.at {
                if m.api.find(ver).is_none() {
                    errors.push(format!(
                        "{}.{name}: `at.\"{ver}\"` is not a declared version of the API",
                        m.service
                    ));
                    continue;
                }
                let input_changed = !shape.input.is_empty() && shape.input != meth.input;
                let output_changed = !shape.output.is_empty() && shape.output != meth.output;
                if !input_changed && !output_changed {
                    errors.push(format!(
                        "{}.{name}: `at.\"{ver}\"` declares the same shape as the current one. \
                         A version only gets declared where something CHANGED; equal, it is an \
                         adapter that copies and a version nobody needed",
                        m.service
                    ));
                    continue;
                }
                match &shape.adapter {
                    None => {
                        let changed: Vec<&str> = shape
                            .output
                            .keys()
                            .filter(|k| meth.output.get(*k) != shape.output.get(*k))
                            .chain(
                                meth.output
                                    .keys()
                                    .filter(|k| !shape.output.contains_key(*k)),
                            )
                            .map(|s| s.as_str())
                            .collect();
                        errors.push(format!(
                            "{}.{name}: `at.\"{ver}\"` changes the shape and declares no \
                             `adapter`. What changed: {}. Without somebody translating it, \
                             whoever pinned that version receives the new shape and finds out \
                             when it breaks",
                            m.service,
                            if changed.is_empty() {
                                "the input".to_string()
                            } else {
                                changed.join(", ")
                            }
                        ));
                    }
                    Some(a) => {
                        if let Some(prev) = adapters.insert(a.as_str(), ver.as_str()) {
                            errors.push(format!(
                                "{}.{name}: `{a}` adapts `{prev}` and `{ver}`. One adapter per \
                                 step: chained, the same function would have to translate two \
                                 different shapes",
                                m.service
                            ));
                        }
                    }
                }
            }
        }
    }

    // A retired version. The point of declaring it is that the announcement
    // stops living in a chat thread: it travels in the response, it is in the
    // OpenAPI, and `verify` can name who is still calling what is about to die.
    for m in ms.iter().filter(|m| !m.external) {
        for (name, meth) in &m.methods {
            for (field, value) in [("deprecated", &meth.deprecated), ("sunset", &meth.sunset)] {
                if let Some(v) = value {
                    if date(v).is_none() {
                        errors.push(format!(
                            "{}.{name}: `{field} = \"{v}\"` is not in YYYY-MM-DD form",
                            m.service
                        ));
                    }
                }
            }
            // A version that dies without ever having been marked as dying: the
            // caller finds out the day the route answers 404.
            if meth.sunset.is_some() && meth.deprecated.is_none() {
                errors.push(format!(
                    "{}.{name}: has `sunset` and no `deprecated`; whoever calls it finds out \
                     the day it stops answering",
                    m.service
                ));
            }
            if let (Some(d), Some(su)) = (
                meth.deprecated.as_deref().and_then(date),
                meth.sunset.as_deref().and_then(date),
            ) {
                if su < d {
                    errors.push(format!(
                        "{}.{name}: it sunsets on {} and is deprecated from {}; there is no \
                         window to migrate in",
                        m.service,
                        meth.sunset.as_deref().unwrap_or_default(),
                        meth.deprecated.as_deref().unwrap_or_default()
                    ));
                }
            }
            // Past its own date and still being served. Same criterion as an
            // expired flag: either it gets removed or the date gets renewed as
            // an explicit decision.
            if let Some(su) = meth.sunset.as_deref().and_then(date) {
                if su < ahora {
                    errors.push(format!(
                        "{}.{name}: sunset on {} and it is still declared. Either the version \
                         goes or the date gets renewed as a decision somebody makes",
                        m.service,
                        meth.sunset.as_deref().unwrap_or_default()
                    ));
                }
            }
            match &meth.successor {
                Some(su) if su == name => {
                    errors.push(format!("{}.{name}: it is its own `successor`", m.service))
                }
                Some(su) if !m.methods.contains_key(su) => errors.push(format!(
                    "{}.{name}: `successor = \"{su}\"` is not a method of the service",
                    m.service
                )),
                None if meth.deprecated.is_some() => warnings.push(format!(
                    "{}.{name}: deprecated with no `successor`; whoever calls it learns that \
                     it is dying and not where to go",
                    m.service
                )),
                _ => {}
            }
            if meth.deprecated.is_some() && meth.sunset.is_none() {
                warnings.push(format!(
                    "{}.{name}: deprecated with no `sunset`; a deprecation with no date does \
                     not end, and the version stays up for years",
                    m.service
                ));
            }
        }
    }
    // And the one that only a platform-wide view can see: somebody still calls
    // what is about to die. Inside a single repo this is a grep; across twenty
    // services it is the question nobody can answer.
    for m in ms.iter().filter(|m| !m.external) {
        for d in &m.depends {
            let tgt = d.target();
            if let Some(sig) = ms
                .iter()
                .find(|o| o.service == tgt)
                .and_then(|o| o.methods.get(&d.method))
            {
                if sig.retiring() {
                    warnings.push(format!(
                        "{} calls {tgt}.{}, which is deprecated{}{}",
                        m.service,
                        d.method,
                        sig.sunset
                            .as_deref()
                            .map(|s| format!(" and sunsets on {s}"))
                            .unwrap_or_default(),
                        sig.successor
                            .as_deref()
                            .map(|s| format!("; the successor is `{s}`"))
                            .unwrap_or_default()
                    ));
                }
            }
        }
    }

    // A scope in the catalogue that no method demands: it can be granted to
    // somebody and it guards nothing, which is the worst kind of permission —
    // it looks like a control and is a label.
    if let Some(api) = ms.iter().find(|m| !m.external).map(|m| &m.api) {
        for sc in &api.scopes {
            let used = ms
                .iter()
                .filter(|m| !m.external)
                .any(|m| m.methods.values().any(|me| me.scopes.contains(sc)));
            if !used {
                warnings.push(format!(
                    "[api] the scope `{sc}` is declared and no method demands it. It can be \
                     granted and it guards nothing"
                ));
            }
        }
    }

    // Declared failures. A method's failures are part of its contract, and the
    // generated client acts on them: it does not retry what the callee declared
    // as not retriable. Which means a wrong declaration is worse than none —
    // it silences a retry that would have worked — so these rules are strict.
    for m in ms.iter().filter(|m| !m.external) {
        for (name, meth) in &m.methods {
            let mut seen: IndexMap<String, ()> = IndexMap::new();
            for f in &meth.errors {
                if !f.well_formed() {
                    errors.push(format!(
                        "{}.{name}: error code `{}` is not snake_case; the code travels in \
                         `problem+json` and gets compared as a literal",
                        m.service, f.code
                    ));
                }
                if seen.insert(f.code.clone(), ()).is_some() {
                    errors.push(format!(
                        "{}.{name}: declares `{}` twice; whichever status and `retriable` \
                         won would depend on the order",
                        m.service, f.code
                    ));
                }
                if !(400..600).contains(&f.status) {
                    errors.push(format!(
                        "{}.{name}.{}: `status = {}` is not a failure; a declared error has \
                         to be 4xx or 5xx",
                        m.service, f.code, f.status
                    ));
                }
                // A 4xx says the request is what is wrong: sending the same
                // request again ends the same way. The exceptions are the three
                // that mean "not yet": 408, 425 and 429.
                if f.retriable
                    && (400..500).contains(&f.status)
                    && !RETRIABLE_4XX.contains(&f.status)
                {
                    errors.push(format!(
                        "{}.{name}.{}: `retriable = true` on {}; a 4xx is the request's fault \
                         and retrying it ends the same. Retriable 4xx: {}",
                        m.service,
                        f.code,
                        f.status,
                        RETRIABLE_4XX
                            .iter()
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            // A public mutation that declares no failure hands every caller the
            // same 500 for a declined card and for a database that is down.
            if meth.auth.as_deref() == Some("public") && meth.mutating() && meth.errors.is_empty() {
                warnings.push(format!(
                    "{}.{name}: public mutation with no declared `errors`; every caller \
                     invents its own reading of a 500",
                    m.service
                ));
            }
        }
    }
    // Retries against a method whose every declared failure is final: the
    // generated client will not retry them, so the budget only buys attempts
    // against what nobody declared.
    for m in ms.iter().filter(|m| !m.external) {
        for d in &m.depends {
            if d.retries == 0 {
                continue;
            }
            let tgt = d.target();
            if let Some(sig) = ms
                .iter()
                .find(|o| o.service == tgt)
                .and_then(|o| o.methods.get(&d.method))
            {
                if !sig.errors.is_empty() && !sig.errors.iter().any(|f| f.retriable) {
                    warnings.push(format!(
                        "{} retries {tgt}.{} {} times and every failure it declares is final; \
                         the retries only apply to what nobody declared",
                        m.service, d.method, d.retries
                    ));
                }
            }
        }
    }

    // How long the events are kept. A table of events grows forever, and the
    // first symptom is the bill while the second is a query that times out.
    for m in ms.iter().filter(|m| !m.external) {
        let a = &m.analytics;
        if !a.export {
            if a.retention_days.is_some() || !a.retention.is_empty() {
                errors.push(format!(
                    "{}: it declares retention and does not export. There is no table to keep \
                     anything in",
                    m.service
                ));
            }
            continue;
        }
        if a.retention_days.is_none() && a.retention.is_empty() && !m.emits.is_empty() {
            warnings.push(format!(
                "{}: exports {} events with no `retention_days`. A table of events grows \
                 forever, and the first symptom is the bill while the second is a query that \
                 times out",
                m.service,
                m.emits.len()
            ));
        }
        for (ev, days) in &a.retention {
            if !m.emits.contains_key(ev) {
                errors.push(format!(
                    "{}: `[analytics.retention]` names `{ev}`, which this service does not \
                     emit. Retention is decided by whoever owns the event",
                    m.service
                ));
            }
            if *days <= 0 {
                errors.push(format!(
                    "{}: `retention.\"{ev}\"` is {days} days",
                    m.service
                ));
            }
        }
        if a.retention_days.is_some_and(|d| d <= 0) {
            errors.push(format!(
                "{}: `retention_days` is not a number of days",
                m.service
            ));
        }
        // And the one that matters: a metric that asks for more history than
        // the table keeps answers zero for the part that was deleted, and zero
        // reads exactly like nothing having happened.
        for (name, mt) in &m.metrics {
            let needs = match mt.window.as_str() {
                "1h" => 1,
                "1d" => 1,
                "1w" => 7,
                "1mo" => 31,
                _ => 1,
            };
            for ev in &mt.on {
                let Some(keeps) = a.keeps(ev) else { continue };
                if keeps < needs {
                    errors.push(format!(
                        "{}.{name}: the metric groups by {} and `{ev}` is kept {keeps} days. \
                         The window that falls outside answers zero, and zero reads exactly \
                         like nothing having happened",
                        m.service, mt.window
                    ));
                }
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
        for (name, sg) in &m.saga {
            if sg.steps.is_empty() {
                errors.push(format!(
                    "{svc}.{name}: a saga with no steps coordinates nothing"
                ));
                continue;
            }

            // The trigger has to exist: a saga nobody starts is generated code that
            // never runs.
            match &sg.on {
                None => errors.push(format!(
                    "{svc}.{name}: no `on`. A saga is started by a method of its own or by a \
                     consumed event, and without saying which, the coordinator gets generated \
                     and never runs"
                )),
                Some(on) => {
                    if !m.methods.contains_key(on) && !m.consumes.contains_key(on) {
                        errors.push(format!(
                            "{svc}.{name}: started by `{on}`, which is neither a method of `{svc}` \
                             nor an event it consumes"
                        ));
                    }
                }
            }

            // The progress has to be on disk. A coordinator that loses a saga
            // halfway neither finishes nor compensates it: the steps already
            // taken stay applied forever and nobody knows which ones they were.
            let table = Saga::table(name);
            match esquemas.get(svc) {
                None => errors.push(format!(
                    "{svc}.{name}: no migrations, and the saga needs the `{table}` table to \
                     survive a restart of the coordinator"
                )),
                Some(tablas) => match tablas.get(&table) {
                    None => errors.push(format!(
                        "{svc}.{name}: the `{table}` table is missing. Without it, a restart mid-saga \
                         leaves the steps already taken applied and with no record of which \
                         ones: it can neither finish nor compensate"
                    )),
                    Some(t) => {
                        // `datos` and `actualizado` are not decoration: without the
                        // envelope that started it, the call cannot be rebuilt
                        // on resume, and without the timestamp the sweep cannot
                        // tell a stranded saga from one still on its way.
                        for (col, what) in [
                            ("id", "the flow's id"),
                            ("step", "how far it got"),
                            ("status", "whether the step was attempted or completed"),
                            ("data", "the envelope that started it, so it can be resumed"),
                            ("updated", "when it last moved, for the sweep"),
                        ] {
                            match t.col(col) {
                                None => errors.push(format!(
                                    "{svc}.{name}: `{table}` has no `{col}` column: that is where {what} goes"
                                )),
                                Some(c) => {
                                    // A wrong type raises no error: it gives a
                                    // comparison that compiles and compares wrong.
                                    let esperado = match col {
                                        "data" => "json",
                                        "updated" => "timestamp",
                                        _ => continue,
                                    };
                                    if !c.ty.to_lowercase().contains(esperado) {
                                        errors.push(format!(
                                            "{svc}.{name}: `{table}.{col}` is `{}` and has to \
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
            for (i, step) in sg.steps.iter().enumerate() {
                // Every reference is resolved against the manifests, not against
                // good faith: a misspelled `undo` is a compensation that does
                // not exist, and it gets discovered the day it is needed.
                let mut resolver = |field: &str, r: &str| -> Option<(u32, &Method)> {
                    let Some((s, met)) = Step::parts(r) else {
                        errors.push(format!(
                            "{svc}.{name}.{field}: `{r}` is not in `service.method` form"
                        ));
                        return None;
                    };
                    let Some(otro) = known.get(s) else {
                        errors.push(format!(
                            "{svc}.{name}.{field}: `{r}` points at `{s}`, which does not exist"
                        ));
                        return None;
                    };
                    let Some(me) = otro.methods.get(met) else {
                        errors.push(format!(
                            "{svc}.{name}.{field}: `{s}` does not offer `{met}`"
                        ));
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
                            "{svc}.{name}.{field}: uses `{r}` without declaring it in \
                             `[[depends]]`. The resilient client —timeout, retries, breaker— \
                             comes from there, and without it the saga has nothing to call with"
                        ));
                    }
                    // The step's budget is the CALLER's, not the one the other
                    // service declares for itself, and with the retries inside:
                    // the coordinator waits for what `[[depends]]` says.
                    let unitario = dep
                        .and_then(|d| d.timeout_ms)
                        .or(me.timeout_ms)
                        .unwrap_or(0);
                    let intentos = dep.map(|d| d.retries + 1).unwrap_or(1);
                    Some((unitario * intentos, me))
                };

                if let Some((ms, _)) = resolver("do", &step.call) {
                    presupuesto += ms;
                }

                match &step.undo {
                    // A deliberate carve-out: if the LAST step fails, there is
                    // nothing of its own to undo. Demanding a compensation for
                    // it would be a false positive, and a rule with false
                    // positives gets silenced wholesale.
                    None if i == ultimo => {}
                    None => errors.push(format!(
                        "{svc}.{name}: step {} (`{}`) has no `undo`, and it is not the \
                         last one. If a later step fails, this one stays applied forever: that \
                         is not a saga, it is a dual-write with more steps",
                        i + 1,
                        step.call
                    )),
                    Some(u) => {
                        if let Some((ms, me)) = resolver("undo", u) {
                            // The compensation gets retried until it lands: there
                            // is nothing behind it. One that is not idempotent
                            // applies the effect twice.
                            if !me.idempotent {
                                errors.push(format!(
                                    "{svc}.{name}: `{u}` compensates step {} and is not \
                                     `idempotent`. A compensation gets retried until it lands \
                                     —there is nothing behind it— and retrying one that is not \
                                     idempotent applies the effect twice",
                                    i + 1
                                ));
                            }
                            presupuesto += ms;
                        }
                        if *u == step.call {
                            errors.push(format!("{svc}.{name}: step {} compensates itself", i + 1));
                        }
                    }
                }
            }

            // The same kind of arithmetic as the connections', and the same
            // underlying error: a declared number that does not cover the sum of
            // the ones already declared.
            match sg.timeout_ms {
                Some(tope) if presupuesto > tope => errors.push(format!(
                    "{svc}.{name}: `timeout_ms = {tope}` and the steps plus their \
                     compensations add up to {presupuesto}ms. Giving up while a step is still \
                     in flight leaves the coordinator compensating something that later \
                     succeeds"
                )),
                Some(_) => {}
                None => warnings.push(format!(
                    "{svc}.{name}: no `timeout_ms`. A saga with no time budget stays in \
                     flight until somebody looks at it"
                )),
            }

            // A saga is eventual consistency by construction: between the first
            // step and the last the system passes through states no invariant
            // describes. Promising CP on top is the same contradiction as reading
            // from a replica and promising CP.
            if !m.cap.eventual() {
                errors.push(format!(
                    "{svc}.{name}: coordinates a saga with `consistency = \"strong\"`. \
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
        for (name, ag) in &m.aggregate {
            if ag.events.is_empty() {
                errors.push(format!(
                    "{svc}.{name}: an aggregate with no events has no state to rebuild"
                ));
            }
            // An aggregate founded on an event the service does not emit is an
            // aggregate nobody can fill.
            for ev in &ag.events {
                if !m.emits.contains_key(ev) {
                    errors.push(format!(
                        "{svc}.{name}: is founded on `{ev}`, which this service does not declare it \
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
                        "{svc}.{name}: governed by `[machine.{mac}]`, which does not exist"
                    )),
                    Some(maq) => {
                        for ev in &ag.events {
                            if !maq
                                .transitions
                                .values()
                                .any(|t| t.emits.as_ref() == Some(ev))
                            {
                                errors.push(format!(
                                    "{svc}.{name}: `{ev}` belongs to the aggregate and no \
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
                    "{svc}.{name}: an aggregate whose events get published needs \
                     `[patterns] outbox = true`. The stream is already durable, so publishing \
                     inline leaves a window where the event is recorded and nobody received \
                     it, and publishing before recording leaves the opposite. The handoff goes \
                     in the SAME transaction as the append"
                ));
            }

            let table = Aggregate::table(name);
            match esquemas.get(svc).and_then(|t| t.get(&table)) {
                None => errors.push(format!(
                    "{svc}.{name}: the `{table}` table is missing. The state IS the stream, and \
                     without the table there is nowhere to put it"
                )),
                Some(t) => {
                    for (col, what) in [
                        (
                            "stream_id",
                            "which instance of the aggregate this event belongs to",
                        ),
                        ("version", "its position in the stream"),
                        ("type", "which of the declared events it is"),
                        ("data", "su contenido"),
                    ] {
                        if !t.has(col) {
                            errors.push(format!(
                                "{svc}.{name}: `{table}` has no `{col}` column: that is where {what} goes"
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
                            "{svc}.{name}: `{table}` has no UNIQUE on (stream_id, version). Two \
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
                let text = std::fs::read_to_string(&f)
                    .unwrap_or_default()
                    .to_lowercase();
                let nombre_archivo = f
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                for verbo in ["update", "delete from", "truncate"] {
                    // look for the verb AND the table in the same statement
                    for sent in text.split(';') {
                        let limpio: String = sent
                            .lines()
                            .filter(|l| !l.trim_start().starts_with("--"))
                            .collect::<Vec<_>>()
                            .join(" ");
                        if limpio.contains(verbo) && limpio.contains(&table) {
                            errors.push(format!(
                                "{svc}/{nombre_archivo}: `{}` on `{table}`, which is the stream of \
                                 `{name}`. A stream is append-only: changing a past event \
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
                let snapshots = Aggregate::snapshots(name);
                match esquemas.get(svc).and_then(|t| t.get(&snapshots)) {
                    None => errors.push(format!(
                        "{svc}.{name}: `snapshot_every = {}` with no `{snapshots}` table",
                        ag.snapshot_every
                    )),
                    Some(t) => {
                        for (col, what, kind) in [
                            ("stream_id", "which instance the snapshot is of", ""),
                            ("version", "how far into the stream it covers", ""),
                            ("state", "the computed state", "json"),
                            (
                                "rules",
                                "which rules version it was computed with: without this column, \
                                 an old snapshot gets rehydrated with new rules and gives a \
                                 state that no longer matches replaying the stream, with no \
                                 error at all",
                                "",
                            ),
                        ] {
                            match t.col(col) {
                                None => errors.push(format!(
                                    "{svc}.{name}: `{snapshots}` has no `{col}` column: that is where {what} goes"
                                )),
                                Some(c) if !kind.is_empty() && !c.ty.to_lowercase().contains(kind) => {
                                    errors.push(format!(
                                        "{svc}.{name}: `{snapshots}.{col}` is `{}` and has to be \
                                         {kind}",
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
                                "{svc}.{name}: `snapshot_every = 1` stores one snapshot per event: that \
                                 is not a cache, it is a second copy of the stream with twice \
                                 the writes"
                            ));
                        }
                    }
                }
            }
        }

        // Read models.
        for (name, vi) in &m.view {
            if vi.on.is_empty() {
                errors.push(format!(
                    "{svc}.{name}: a view with no events is built from nothing"
                ));
            }
            for ev in &vi.on {
                // It can consume its own events or another service's; what it
                // cannot do is consume one nobody emits.
                if !emitters.contains_key(ev.as_str()) {
                    errors.push(format!(
                        "{svc}.{name}: is built from `{ev}`, which nobody emits"
                    ));
                }
                // And if it belongs to another service it has to be declared in
                // `[consumes]`: `axon infra` builds the delivery from there.
                let propio = m.emits.contains_key(ev.as_str());
                if !propio && !m.consumes.contains_key(ev.as_str()) {
                    errors.push(format!(
                        "{svc}.{name}: uses `{ev}`, from another service, without declaring it in \
                         `[consumes]`. The subscription comes from there, and without it the \
                         view gets generated and never receives anything"
                    ));
                }
            }
            let tablas = esquemas.get(svc);
            let table = vi.table(name);
            if tablas.and_then(|t| t.get(&table)).is_none() {
                errors.push(format!(
                    "{svc}.{name}: the `{table}` table, where the view lives, is missing"
                ));
            }
            let cp = View::checkpoint(name);
            match tablas.and_then(|t| t.get(&cp)) {
                None => errors.push(format!(
                    "{svc}.{name}: the `{cp}` table is missing. With nowhere to record how far it \
                     got, a restart either reprocesses from the beginning or skips what it did \
                     not get to apply; both give a wrong view and neither raises an error"
                )),
                Some(t) => {
                    for (col, what) in [
                        ("view_name", "which of the views this is"),
                        ("stream_id", "which stream"),
                        ("position", "how far into THAT stream it got"),
                    ] {
                        if !t.has(col) {
                            errors.push(format!(
                                "{svc}.{name}: `{cp}` has no `{col}` column: that is where {what} goes"
                            ));
                        }
                    }
                    // Without `stream_id` in the key, one stream overwrites
                    // another's position. An event's version is its position
                    // inside ITS stream: a single number for the whole view
                    // seems to work while there is one stream, and stops
                    // identifying anything as soon as there are two.
                    let por_flujo = t.uniques.iter().any(|u| {
                        u.iter().any(|c| c == "stream_id") && u.iter().any(|c| c == "view_name")
                    });
                    if !por_flujo && t.has("stream_id") {
                        errors.push(format!(
                            "{svc}.{name}: `{cp}` has no key on (view_name, stream_id). One stream would \
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
                let shadow = format!("{}_shadow", table);
                match (
                    tablas.and_then(|t| t.get(&table)),
                    tablas.and_then(|t| t.get(&shadow)),
                ) {
                    (Some(_), None) => errors.push(format!(
                        "{svc}.{name}: it can be rebuilt and `{shadow}` is missing. Rebuilding in \
                         place leaves the view incomplete while it runs, and it keeps being \
                         read: whoever asks gets fewer rows than there are, with no error"
                    )),
                    (Some(viva), Some(som)) => {
                        for c in &viva.cols {
                            match som.col(&c.name) {
                                None => errors.push(format!(
                                    "{svc}.{name}: `{shadow}` has no `{}` column, which `{table}` \
                                     does. The swap would leave a view without that data, and \
                                     only then would it show",
                                    c.name
                                )),
                                Some(o) if o.ty != c.ty => errors.push(format!(
                                    "{svc}.{name}: `{shadow}.{}` is `{}` and in `{table}` it is \
                                     `{}`. On the swap, the view changes type without anything \
                                     saying so",
                                    c.name, o.ty, c.ty
                                )),
                                _ => {}
                            }
                        }
                        for c in &som.cols {
                            if !viva.has(&c.name) {
                                warnings.push(format!(
                                    "{svc}.{name}: `{shadow}.{}` is not in `{table}`. It is \
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
                    "{svc}.{name}: a read model with `consistency = \"strong\"`. The view gets \
                     filled after the event happened: what it serves is stale by definition"
                ));
            }
            match (vi.max_staleness_ms, m.cap.max_staleness_ms) {
                (Some(v), Some(tope)) if v > tope => errors.push(format!(
                    "{svc}.{name}: the view allows {v}ms of lag and the service declared a limit \
                     of {tope}ms. The service cannot honour what it promised while serving \
                     from a view older than its own budget"
                )),
                (None, _) => warnings.push(format!(
                    "{svc}.{name}: no `max_staleness_ms`. With no lag budget, nobody can say \
                     whether the view it served was acceptably stale"
                )),
                _ => {}
            }
        }
    }

    // Business metrics. What a funnel answers is derivable from the causal chain;
    // this is not, so the whole value of declaring it is that these things become
    // refutable instead of being a query somebody pasted into a dashboard.
    let emitted: IndexMap<&str, &Manifest> = ms
        .iter()
        .filter(|m| !m.external)
        .flat_map(|m| m.emits.keys().map(move |e| (e.as_str(), m)))
        .collect();
    let mut metric_owner: IndexMap<String, String> = IndexMap::new();
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        for (name, mt) in &m.metrics {
            // A metric lives in the warehouse and reads the exported tables: with
            // `export = false` there is nothing there, and the view gets applied
            // over tables that carry no rows.
            if !m.analytics.export {
                errors.push(format!(
                    "{svc}.{name}: a metric with `[analytics] export = false` reads tables that \
                     carry nothing. It gets applied and answers zero forever, which is \
                     indistinguishable from a business that sold nothing"
                ));
            }
            // The view's name is global to the dataset: two services with the same
            // metric name overwrite each other's view, and the second one wins in
            // silence.
            if let Some(prev) = metric_owner.insert(name.clone(), svc.clone()) {
                errors.push(format!(
                    "{svc}.{name}: `{}` is also declared by {prev}, and both land on the same \
                     view in the warehouse. Whichever is applied second overwrites the first \
                     without an error",
                    Metric::view(name)
                ));
            }
            if !AGGREGATIONS.contains(&mt.kind.as_str()) {
                errors.push(format!(
                    "{svc}.{name}: `kind = \"{}\"` is not an aggregation; use {}. Those are the \
                     ones that mean the same thing in the three warehouses",
                    mt.kind,
                    AGGREGATIONS.join(", ")
                ));
            }
            if !WINDOWS.contains(&mt.window.as_str()) {
                errors.push(format!(
                    "{svc}.{name}: `window = \"{}\"` is not a bucket; use {}",
                    mt.window,
                    WINDOWS.join(", ")
                ));
            }
            match (mt.adds_up(), &mt.field) {
                (true, None) => errors.push(format!(
                    "{svc}.{name}: `{}` with no `field`; there is nothing to add up",
                    mt.kind
                )),
                (false, Some(f)) => warnings.push(format!(
                    "{svc}.{name}: `count` with `field = \"{f}\"`. A count counts rows: the field \
                     is ignored, and nobody reading the declaration would guess so"
                )),
                _ => {}
            }
            if mt.on.is_empty() {
                errors.push(format!(
                    "{svc}.{name}: a metric with no `on` reads nothing and answers nothing"
                ));
            }
            for ev in &mt.on {
                let Some(owner) = emitted.get(ev.as_str()) else {
                    errors.push(format!(
                        "{svc}.{name}: reads `{ev}` and nobody emits it. The view gets applied \
                         and counts zero forever"
                    ));
                    continue;
                };
                // The metric reads the exported table, so the event's owner has to
                // export: with `export = false` on the emitter's side, that table
                // does not exist at all.
                if !owner.analytics.export {
                    errors.push(format!(
                        "{svc}.{name}: reads `{ev}`, and {} declares `[analytics] export = \
                         false`: that event has no table in the warehouse",
                        owner.service
                    ));
                    continue;
                }
                let fields = &owner.emits[ev];
                // The field it adds up has to BE a number in the schema its emitter
                // declares. A sum over a string is not an error in any warehouse:
                // one refuses it at apply time and another answers zero, and zero
                // reads exactly like "nothing was sold".
                if let (true, Some(f)) = (mt.adds_up(), &mt.field) {
                    match fields.iter().find(|(k, _)| normalize(k) == normalize(f)) {
                        None => errors.push(format!(
                            "{svc}.{name}: adds up `{f}`, which `{ev}` does not declare. That \
                             column does not exist in that table"
                        )),
                        Some((_, kind)) if !NUMERIC.contains(&kind.as_str()) => {
                            errors.push(format!(
                                "{svc}.{name}: adds up `{f}`, which is `{kind}` in `{ev}`. A sum \
                                 over something that is not a number is refused by one \
                                 warehouse and answers zero in another, and zero reads like \
                                 nothing happened"
                            ))
                        }
                        _ => {}
                    }
                }
                for dim in &mt.by {
                    // A dimension missing in one of the events silently becomes a
                    // NULL group: the metric keeps answering, with a row nobody can
                    // attribute to anything.
                    // A `money` field is two columns in the warehouse, so
                    // `total.amount` and `total.currency` are dimensions even
                    // though the contract declares one field called `total`.
                    let money_part = dim.rsplit_once('.').is_some_and(|(head, part)| {
                        matches!(part, "amount" | "currency")
                            && fields
                                .iter()
                                .any(|(k, t)| t == "money" && normalize(k) == normalize(head))
                    });
                    if !money_part && !fields.iter().any(|(k, _)| normalize(k) == normalize(dim)) {
                        errors.push(format!(
                            "{svc}.{name}: groups by `{dim}`, which `{ev}` does not declare. \
                             That turns into a NULL group, and a metric with a NULL group is \
                             one nobody can read"
                        ));
                    }
                    // And a personal field is not a dimension. Hashed or not, one
                    // row per person with a `GROUP BY` is a lookup table, and the
                    // warehouse is where it would live longest.
                    if is_pii(&owner.pii, dim) {
                        errors.push(format!(
                            "{svc}.{name}: groups by `{dim}`, which {} declares as `pii`. One \
                             row per person is not a metric, and hashing it does not change \
                             that: the hash identifies the same person across tables",
                            owner.service
                        ));
                    }
                }
            }
        }
    }

    // state machines: dead states, unreachable ones and phantom triggers
    for m in ms.iter().filter(|m| !m.external) {
        for (name, mac) in &m.machine {
            let states = mac.states();
            if !states.contains(&mac.initial) {
                errors.push(format!(
                    "{}.{name}: initial state `{}` appears in no transition",
                    m.service, mac.initial
                ));
            }
            // reachability from the initial state
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
                        "{}.{name}: state `{st}` is unreachable from `{}`",
                        m.service, mac.initial
                    ));
                }
                let sale = mac.transitions.values().any(|t| t.from.contains(st));
                if !sale && !mac.final_states.contains(st) {
                    errors.push(format!(
                        "{}.{name}: `{st}` is not final and has no way out; it is a deadlock",
                        m.service
                    ));
                }
            }
            for (act, t) in &mac.transitions {
                if !states.contains(&t.to) {
                    errors.push(format!(
                        "{}.{name}.{act}: unknown target `{}`",
                        m.service, t.to
                    ));
                }
                // the trigger has to actually exist
                if !m.methods.contains_key(&t.on) && !m.consumes.contains_key(&t.on) {
                    errors
                        .push(format!(
                        "{}.{name}.{act}: triggered by `{}`, which is neither a method nor a consumed event",
                        m.service, t.on));
                }
                if let Some(ev) = &t.emits {
                    if !m.emits.contains_key(ev) {
                        errors.push(format!(
                            "{}.{name}.{act}: emits `{ev}`, which the service does not declare it emits",
                            m.service
                        ));
                    }
                }
                if let Some(c) = &t.compensates {
                    if !mac.transitions.contains_key(c) {
                        errors.push(format!(
                            "{}.{name}.{act}: compensates `{c}`, which does not exist",
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
                errors.push(format!("{svc} consumes {ev} but nobody emits it"));
            }
        }
        for d in &m.depends {
            let tgt = d.target();
            match known.get(tgt) {
                None => errors.push(format!("{svc} depends on {tgt}, with no known manifest")),
                Some(t) if !t.methods.contains_key(&d.method) => errors.push(format!(
                    "{svc} calls {tgt}.{}, which {tgt} does not expose",
                    d.method
                )),
                _ => {}
            }
            if d.timeout_ms.is_none() {
                errors.push(format!(
                    "{svc} -> {tgt}.{}: no `timeout_ms`; a network call with no time budget \
                     propagates the other side's outage",
                    d.method
                ));
            }
            if d.retries > 0
                && !known
                    .get(tgt)
                    .is_some_and(|t| t.methods.get(&d.method).is_some_and(|m| m.is_idempotent()))
            {
                errors.push(format!(
                    "{svc} retries {tgt}.{}, which is not declared idempotent",
                    d.method
                ));
            }
            if d.retries > 0 && !d.breaker {
                warnings.push(format!(
                    "{svc} -> {tgt}.{}: retries with no `breaker = true`; retries amplify \
                     the other side's outage",
                    d.method
                ));
            }
        }
    }

    // migrations: expand -> migrate -> contract, and a deterministic order
    for m in ms {
        // Two migrations with the same version: Flyway refuses to apply EITHER,
        // so the deploy falls over with the database half migrated. It catches
        // that at startup; here it is caught at commit time, which is when the
        // file can be renamed without any hurry.
        let mut vistas: IndexMap<String, String> = IndexMap::new();
        for f in migrations_of(m) {
            let name = f
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let Some((ver, _)) = name.split_once('_') else {
                continue;
            };
            if let Some(other) = vistas.get(ver) {
                errors.push(format!(
                    "{}: `{name}` and `{other}` share the version `{ver}`. Flyway applies neither \
                     of them and the deploy falls over with the database half migrated",
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
                    "{}/{name}: destructive migration not marked as `.contract.sql` \
                     (expand -> migrate -> contract)",
                    m.service
                ));
            }
            // `001_x.sql`: three digits and an underscore. A regex for this
            // would be a whole dependency for three characters.
            let numerado = {
                let b = name.as_bytes();
                b.len() > 3 && b[..3].iter().all(u8::is_ascii_digit) && b[3] == b'_'
            };
            if !numerado {
                warnings.push(format!(
                    "{}/{name}: no numeric prefix, so the order is not deterministic",
                    m.service
                ));
            }
        }
    }

    // database per service: no FK crosses the boundary
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
                            "{svc}.{t}.{}: an FK to {fk} crosses the service boundary \
                             (owner: {owner}); store the id, not an FK",
                            c.name
                        ));
                    }
                }
            }
        }
    }

    for (ev, (owner, _)) in &emitters {
        if !ms.iter().any(|m| m.consumes.contains_key(*ev)) {
            warnings.push(format!("{ev} ({owner}) has no consumers"));
        }
    }
    Report { errors, warnings }
}
