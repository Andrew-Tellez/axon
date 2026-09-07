//! Loading, discovery, and the schema derived from the migrations.
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub type Fields = IndexMap<String, String>;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Consume {
    pub handler: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Method {
    #[serde(rename = "in", default)]
    pub input: Fields,
    #[serde(rename = "out", default)]
    pub output: Fields,
    /// HTTP exposure: "POST /payments". Without it the method is internal RPC only.
    pub http: Option<String>,
    /// Retryable with no duplicated effects. Mandatory on mutating methods.
    #[serde(default)]
    pub idempotent: bool,
    /// Who may call it from the edge: "public" or "required". Deliberately
    /// without a default: an exposed route with this undecided is an incident.
    pub auth: Option<String>,
    /// Requests per minute at the gateway.
    pub rate_limit: Option<u32>,
    /// Time budget at the edge.
    pub timeout_ms: Option<u32>,
    /// Returns a collection: forces cursor pagination.
    #[serde(default)]
    pub paginated: bool,
}

impl Method {
    pub fn verb(&self) -> Option<&str> {
        self.http.as_ref()?.split_whitespace().next()
    }
    pub fn path(&self) -> Option<&str> {
        self.http.as_ref()?.split_whitespace().nth(1)
    }
    pub fn mutating(&self) -> bool {
        matches!(self.verb(), Some("POST" | "PUT" | "PATCH" | "DELETE"))
    }
    /// GET/HEAD are by definition; everything else has to be declared.
    pub fn is_idempotent(&self) -> bool {
        self.idempotent || !self.mutating()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Depend {
    pub service: Option<String>,
    pub external: Option<String>,
    pub method: String,
    /// The concrete handler making the call; sharpens the sequence diagram.
    pub via: Option<String>,
    /// Every network call has a time budget. Mandatory.
    pub timeout_ms: Option<u32>,
    #[serde(default)]
    pub retries: u32,
    /// Cuts the cascade when the other side goes down.
    #[serde(default)]
    pub breaker: bool,
}

impl Depend {
    pub fn target(&self) -> &str {
        self.service
            .as_deref()
            .or(self.external.as_deref())
            .unwrap_or("?")
    }
}

/// The side of the CAP theorem this service picks.
///
/// Partition tolerance is not a choice: in a distributed system the network
/// partitions, full stop. What you choose is what to do while it is
/// partitioned, and that decision changes the isolation level, the read
/// topology, and whether the generated code forces you to write a degraded
/// path.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Cap {
    /// "strong" (CP): rather than serve stale data, serve nothing.
    /// "eventual" (AP): serve something stale rather than nothing.
    pub consistency: String,
    /// "reject" fails closed; "degrade" forces declaring what gets served.
    pub on_partition: String,
    /// Staleness budget. Without a number, "eventual" means nothing.
    pub max_staleness_ms: Option<u32>,
    /// `true` when somebody made the choice; `false` when it is the default.
    #[serde(skip)]
    pub declared: bool,
}

impl Default for Cap {
    fn default() -> Self {
        // The safe pair: fails closed. It is a default, not a decision, and
        // `verify` says so when nobody made it.
        Self {
            consistency: "strong".into(),
            on_partition: "reject".into(),
            max_staleness_ms: None,
            declared: false,
        }
    }
}

impl Cap {
    pub fn eventual(&self) -> bool {
        self.consistency == "eventual"
    }
    pub fn degrades(&self) -> bool {
        self.on_partition == "degrade"
    }
    /// Isolation to match: paying twice costs more than retrying.
    pub fn isolation(&self) -> &str {
        if self.eventual() {
            "READ COMMITTED"
        } else {
            "SERIALIZABLE"
        }
    }
}

/// What happens to this service's events once they reach the warehouse.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Analytics {
    /// `false` leaves the service out of the export.
    pub export: bool,
    /// What to do with fields declared `pii` when exporting them:
    /// `"exclude"` does not send them, `"hash"` sends a salted SHA-256.
    ///
    /// The default is to exclude. A warehouse is where personal data lives
    /// longest, gets copied most, and is read by the most people, so the safe
    /// value has to be the one that does not send it.
    pub pii: String,
    /// Which warehouse. This used to be a CLI flag, which allowed generating
    /// the Snowflake schema and deploying infrastructure that carries nothing
    /// there: the schema applied and the tables stayed empty with nothing
    /// saying so. Declared, `axon infra` can wire the ingest —or refuse.
    pub warehouse: String,
}

/// The warehouses a dialect exists for. That the dialect exists does not mean
/// the ingest path exists on every target: that is what `INGEST` says.
pub const WAREHOUSES: [&str; 3] = ["bigquery", "snowflake", "clickhouse"];

/// (target, warehouse) combinations with a wired ingest path. What is not
/// here `axon infra` REFUSES: generating the schema and carrying nothing to
/// the warehouse is the worst outcome, because it applies with no error.
pub const INGEST: [(&str, &str); 5] = [
    // a Pub/Sub subscription straight into BigQuery
    ("gcp", "bigquery"),
    // Firehose to S3, and from there the warehouse loads with its own tooling
    ("aws", "snowflake"),
    ("aws", "clickhouse"),
    // a ClickHouse container and a loader for the envelope log
    ("local", "clickhouse"),
    // a broker consumer, with config generated and checked by `vector validate`
    ("k8s", "clickhouse"),
];

pub fn has_ingest(target: &str, warehouse: &str) -> bool {
    INGEST.contains(&(target, warehouse))
}

impl Default for Analytics {
    fn default() -> Self {
        Self {
            export: true,
            pii: "exclude".into(),
            warehouse: "bigquery".into(),
        }
    }
}

/// The pooler or sharder in front of the database.
///
/// Putting a proxy in the data path changes the subject of almost every
/// connection rule and —most importantly— **breaks tenant isolation if
/// nobody declares it**: in transaction mode the same physical connection is
/// handed to another tenant, and a session GUC that survives returns the
/// previous one's rows with no error.
///
/// Hence almost everything here being mandatory instead of having a
/// comfortable default: the choice has to be somebody's.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Pooler {
    /// `"none"` (no pooler), or `"pgdog"`.
    pub engine: String,
    /// `transaction`, `session` or `statement`. In `transaction` the connection
    /// goes back to the pool on every COMMIT, which is what breaks per-session
    /// isolation.
    pub mode: String,
    /// Shard nodes. `1` is pooler only, no sharding.
    pub shards: u32,
    /// Ceiling on CLIENT connections the pooler accepts. With a pooler in the
    /// middle, the instance arithmetic is compared against this and not
    /// against the engine's ceiling.
    pub max_client_conn: Option<u32>,
    /// Connections the pooler opens to EACH engine.
    pub pool_size: Option<u32>,
    /// Reject every query touching more than one node instead of running it.
    /// Turns each of the sharder's limitations into a loud error.
    pub cross_shard_disabled: bool,
    /// How the tenant is pinned: `"set_local"` is the only thing that is safe
    /// in transaction mode. See the header `axon rls` generates.
    pub tenant_binding: Option<String>,
}

impl Default for Pooler {
    fn default() -> Self {
        Self {
            engine: "none".into(),
            // the safest of the three, not the fastest
            mode: "session".into(),
            shards: 1,
            max_client_conn: None,
            pool_size: None,
            // fail loudly rather than run something the sharder cannot
            // resolve correctly
            cross_shard_disabled: true,
            tenant_binding: None,
        }
    }
}

impl Pooler {
    pub fn active(&self) -> bool {
        self.engine != "none"
    }
}

/// A feature flag.
///
/// What declaring them adds is not the SDK —OpenFeature and flagd already
/// exist— but that the compiler can enforce what nobody enforces: that every
/// flag has an owner and a death date. Code with two hundred stale flags does
/// not have two hundred features: it has two hundred branches nobody tests.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Flag {
    pub owner: Option<String>,
    /// Variants, as in OpenFeature: a flag is not just a boolean. The value
    /// can be a bool, string, number or object, and evaluation returns the
    /// value of a named variant.
    ///
    /// Without `variants`, the flag is the boolean case and the variants are
    /// `on` and `off` —which is what most need and is not worth writing.
    #[serde(default)]
    pub variants: IndexMap<String, serde_json::Value>,
    /// Name of the default variant. With custom variants it is mandatory.
    pub default_variant: Option<String>,
    /// `YYYY-MM-DD`. Past that date, `verify` fails: the flag gets cleaned up
    /// or renewed with an explicit decision.
    pub expires: Option<String>,
    /// The safe value. A new flag on by default is not a rollout.
    #[serde(default)]
    pub default: bool,
    /// Percentage of the gradual rollout, 0..=100.
    pub rollout: Option<u32>,
    /// Field the decision is pinned by. Without it evaluation is per request,
    /// and the SAME entity changes path halfway through a flow.
    pub sticky_by: Option<String>,
    /// Emergency switch: lives indefinitely and has no gradual rollout,
    /// because it goes off whole or it is useless.
    #[serde(default)]
    pub kill_switch: bool,
}

impl Flag {
    /// The effective variants. A flag without `variants` is the boolean case.
    pub fn all_variants(&self) -> IndexMap<String, serde_json::Value> {
        if self.variants.is_empty() {
            let mut v = IndexMap::new();
            v.insert("on".into(), serde_json::Value::Bool(true));
            v.insert("off".into(), serde_json::Value::Bool(false));
            v
        } else {
            self.variants.clone()
        }
    }

    /// The default variant, or the one matching the `default` boolean.
    pub fn default_variant(&self) -> String {
        self.default_variant.clone().unwrap_or_else(|| {
            if self.default {
                "on".into()
            } else {
                "off".into()
            }
        })
    }

    /// The OpenFeature type matching the declared values. Determines the
    /// generated accessor: `getBooleanValue`, `getStringValue`,
    /// `getNumberValue` or `getObjectValue`.
    pub fn kind(&self) -> &'static str {
        match self.all_variants().values().next() {
            Some(serde_json::Value::Bool(_)) | None => "boolean",
            Some(serde_json::Value::String(_)) => "string",
            Some(serde_json::Value::Number(_)) => "number",
            _ => "object",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Patterns {
    #[serde(default)]
    pub outbox: bool,
}

/// A bucket of the service. `public = true` puts it behind a CDN: nothing is
/// served publicly without cache, and nothing private carries one.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Bucket {
    #[serde(default)]
    pub public: bool,
    /// Days after which the object is deleted. Without it a bucket grows forever.
    pub retention_days: Option<u32>,
    /// CDN TTL in seconds; only applies to public buckets.
    pub cache_ttl: Option<u32>,
}

/// Store engines axon supports TODAY.
///
/// It is a closed list on purpose. `state` used to be a free string, so
/// `state = "neo4j"` passed `verify` with no error and generated a Cloud SQL
/// Postgres instance: wrong output, silently, which is the worst failure mode
/// there is.
///
/// The plan is to support more families —time series, graph, columnar,
/// document— and the natural order is the Postgres extensions (TimescaleDB,
/// Apache AGE, pgvector), because they reuse the SQL parser, the migrations,
/// the RLS and the four targets that already exist. Until then, declaring an
/// engine that is not here has to fail and say how to proceed.
pub const ENGINES: [&str; 1] = ["postgres"];

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Infra {
    pub state: Option<String>,
    pub runtime: Option<String>,
    /// Migrations directory: the schema's source of truth.
    pub migrations: Option<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    pub min_instances: Option<u32>,
    pub max_instances: Option<u32>,
    /// The container's HTTP port.
    pub port: Option<u16>,
    /// The service's object storage, by logical name.
    #[serde(default)]
    pub buckets: IndexMap<String, Bucket>,
    /// Connections EACH instance opens. Multiplied by the instance ceiling
    /// is what reaches the engine.
    pub pool_size: Option<u32>,
    /// The engine's connection ceiling. If the product goes past it, the
    /// service falls over from exhaustion when it scales, not when you test it.
    pub max_connections: Option<u32>,
    /// High availability: a standby with automatic failover.
    ///
    /// It is NOT the same as a read replica, and confusing the two is the
    /// most common mistake on the subject. Nobody reads from the standby: it
    /// exists so the service stays up when the primary goes down, and that is
    /// why it does NOT break consistency. A read replica IS read from, it
    /// lags, and that is why it does.
    pub ha: Option<bool>,
    /// Days of backup retention. High availability is not a backup: a standby
    /// replicates the `DROP TABLE` in seconds.
    pub backup_retention_days: Option<u32>,
    /// Point-in-time recovery. The only thing that saves you from a logical
    /// delete, which is what a standby does not save you from.
    pub pitr: Option<bool>,
    /// READ replicas: they are read from, and they lag. Declaring them is
    /// choosing availability over consistency for those reads.
    pub read_replicas: Option<u32>,
    /// Column the table is sharded by across nodes. Every table needs one,
    /// and no FK can cross from a sharded table to one that is not.
    pub shard_key: Option<String>,
    /// Column identifying the tenant. If present, every table needs it and
    /// `axon rls` generates the policy enforcing it.
    pub tenant_column: Option<String>,
    /// Tables that are not business data and carry no tenant.
    #[serde(default)]
    pub tenant_exempt: Vec<String>,
}

/// A transition. The WHAT is portable to any language; the HOW
/// (the handler's body) is always written by a person.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Transition {
    pub from: Vec<String>,
    pub to: String,
    /// Method or event that fires it.
    pub on: String,
    /// Event emitted on completing it.
    pub emits: Option<String>,
    /// The inverse transition, for sagas: what undoes this step.
    pub compensates: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Machine {
    pub initial: String,
    #[serde(default, rename = "final")]
    pub final_states: Vec<String>,
    #[serde(default)]
    pub transitions: IndexMap<String, Transition>,
}

impl Machine {
    pub fn states(&self) -> Vec<String> {
        let mut s = vec![self.initial.clone()];
        for t in self.transitions.values() {
            for f in &t.from {
                if !s.contains(f) {
                    s.push(f.clone());
                }
            }
            if !s.contains(&t.to) {
                s.push(t.to.clone());
            }
        }
        for f in &self.final_states {
            if !s.contains(f) {
                s.push(f.clone());
            }
        }
        s
    }
}

/// A saga step: the action and what undoes it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Step {
    /// `service.method` to invoke. Has to be a declared dependency.
    #[serde(rename = "do")]
    pub call: String,
    /// `service.method` that reverts the step. Only the LAST one may omit it:
    /// if the last one fails, there is nothing of its own to undo.
    pub undo: Option<String>,
}

impl Step {
    /// `("payments", "capturePayment")`. `None` if it does not have the
    /// `service.method` shape.
    pub fn parts(r: &str) -> Option<(&str, &str)> {
        let (svc, met) = r.split_once('.')?;
        (!svc.is_empty() && !met.is_empty() && !met.contains('.')).then_some((svc, met))
    }
}

/// Saga: a sequence of steps across different services, each with its
/// compensation, coordinated by this service.
///
/// What makes it declarable is that the coordinator has no business logic:
/// it calls in order, and if something fails it undoes what it already did in
/// reverse order. That gets generated. What cannot be generated —that the
/// compensation exists, that it is idempotent, that the time budget closes—
/// can be REFUTED, and that is where the errors that cost money live.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Saga {
    /// Own method or consumed event that starts it.
    pub on: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
    /// Budget for the whole flow. It has to cover the sum of the steps:
    /// giving up while a step is still in flight leaves the coordinator
    /// compensating something that later succeeds.
    pub timeout_ms: Option<u32>,
}

impl Saga {
    /// The table where progress lives. Without it a coordinator restart
    /// loses the half-finished saga: it neither finishes nor compensates.
    pub fn table(name: &str) -> String {
        format!("saga_{}", name.to_lowercase())
    }
    /// The route the sweep comes in through. It lives HERE and not in each
    /// generator because two of them concatenate it: the code that serves it
    /// and the scheduler that hits it. When they drifted —the route in English
    /// and the cron still in Spanish— the CronJob applied with no error and
    /// hit a 404 forever: the sweep simply stopped running, and the only thing
    /// that said so was a curl swallowing the failure.
    pub fn sweep_route(name: &str) -> String {
        format!("/internal/saga/{name}/sweep")
    }
}

/// Event sourcing: the state IS the event stream, and what today lives in a
/// row is a projection of that stream.
///
/// What can be generated of this is everything mechanical: the append-only
/// table, the `fold` with one case per declared event —so adding an event
/// breaks the build— and the append with optimistic versioning. What can be
/// REFUTED is what costs dearly: an event the service does not emit, an
/// `UPDATE` on the stream, or the missing UNIQUE that keeps two concurrent
/// writes from overwriting each other without a single error.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Aggregate {
    /// The events making up the state. All of them have to be declared in
    /// `[emits]`: an aggregate cannot be founded on a contract that does not exist.
    #[serde(default)]
    pub events: Vec<String>,
    /// The state machine governing the transitions, if there is one. With it,
    /// the generated `fold` rejects an event arriving out of order instead of
    /// applying it.
    pub machine: Option<String>,
    /// How many events between snapshots. 0 is no snapshots: always rebuild
    /// from the beginning.
    #[serde(default)]
    pub snapshot_every: u32,
    /// Version of the RULES the snapshot was computed with.
    ///
    /// A snapshot is a cache of the `fold`, and if the `fold` changes —a new
    /// rule, a field now accumulated differently— the old snapshots encode the
    /// previous version. Rehydrating from there gives a state that no longer
    /// matches replaying the stream, and that raises no error: it gives a
    /// wrong number.
    ///
    /// Bumping this number invalidates the existing snapshots and makes them
    /// rebuild. It is the only thing turning that silent failure into one
    /// that fixes itself.
    #[serde(default = "one")]
    pub snapshot_version: u32,
}

fn one() -> u32 {
    1
}

impl Aggregate {
    /// The stream's table. Append-only: `verify` blocks any migration that
    /// updates it or deletes from it.
    pub fn table(name: &str) -> String {
        format!("{}_event", name.to_lowercase())
    }
    /// The snapshot table, if snapshots were declared.
    pub fn snapshots(name: &str) -> String {
        format!("{}_snapshot", name.to_lowercase())
    }
    /// The prune route. Same reason as `Saga::sweep_route`: whoever serves it
    /// and whoever schedules it read it from one place.
    pub fn prune_route(name: &str) -> String {
        format!("/internal/aggregate/{name}/prune")
    }
}

/// A business metric over the declared events.
///
/// The funnel that comes out of the causal chain already answers conversion and
/// business latency, because both are derivable from who causes whom. A rate, a
/// sum by dimension or an average is not derivable from anything: somebody has
/// to say which field and grouped by what. Declared here, the metric ends up in
/// the warehouse next to the funnels instead of hand-written in the BI tool,
/// which is where it stops being verifiable.
///
/// What can be REFUTED is what makes it worth declaring: a metric over an event
/// nobody emits, a sum over a field that is not a number, a dimension that does
/// not exist in every event it reads, and —the one that matters— a dimension
/// that is a personal field, because a metric grouped by an email address is not
/// a metric, it is a lookup table with a `GROUP BY`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Metric {
    /// The events it is computed from. All of them have to be declared by
    /// somebody: a metric over an event nobody emits counts nothing forever.
    #[serde(default)]
    pub on: Vec<String>,
    /// `count`, `sum` or `avg`. The three exist with the same syntax in the
    /// three warehouses; a quantile does not, and one that changes name per
    /// dialect would be a metric that means something slightly different in each.
    #[serde(default = "count")]
    pub kind: String,
    /// The field to add up. Mandatory for `sum` and `avg`, and it has to be a
    /// number in the schema its emitter declares. A `money` field is added up by
    /// its amount: `total` reads the `total_amount` column.
    pub field: Option<String>,
    /// The dimensions to group by, on top of the time bucket. They have to exist
    /// in EVERY event of `on`: one missing in one of them silently turns into a
    /// NULL group, and a metric with a NULL group is one nobody can read.
    #[serde(default)]
    pub by: Vec<String>,
    /// The time bucket: `1h`, `1d`, `1w` or `1mo`. Without a bucket a metric is
    /// one number for all of history, which is a total and not a metric.
    #[serde(default = "one_day")]
    pub window: String,
}

fn count() -> String {
    "count".to_string()
}

fn one_day() -> String {
    "1d".to_string()
}

/// The types a `sum` or an `avg` can add up. A string that looks like a number
/// is still a string: the warehouse either refuses the sum or answers zero.
pub const NUMERIC: [&str; 3] = ["int", "float", "money"];

/// The aggregations that mean the same thing in the three dialects.
pub const AGGREGATIONS: [&str; 3] = ["count", "sum", "avg"];

/// The buckets each dialect can express without a per-warehouse expression that
/// would silently mean something else.
pub const WINDOWS: [&str; 4] = ["1h", "1d", "1w", "1mo"];

impl Metric {
    /// The view it lives in. `metric_<name>` so it sits next to `funnel_<event>`
    /// in the same dataset, and so a name that collides between two services is
    /// a collision `verify` can see.
    pub fn view(name: &str) -> String {
        format!("metric_{}", name.to_lowercase())
    }
    /// Whether it adds a field up, which is what makes `field` mandatory.
    pub fn adds_up(&self) -> bool {
        self.kind == "sum" || self.kind == "avg"
    }
}

/// CQRS: a read model built by applying already declared events.
///
/// What declaring it adds is not the code —a projection is a `switch`— but
/// that the compiler enforces what nobody enforces: that the view only
/// consumes events somebody emits, that it has somewhere to record how far
/// it got, and that its staleness fits the one the service already promised.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct View {
    /// The events that build it.
    #[serde(default)]
    pub on: Vec<String>,
    /// The table it lives in. `view_<name>` by default.
    pub table: Option<String>,
    /// How far it may lag. It has to fit in the service's `max_staleness_ms`:
    /// a view staler than that makes the declaration a lie.
    pub max_staleness_ms: Option<u32>,
}

impl View {
    pub fn table(&self, name: &str) -> String {
        self.table
            .clone()
            .unwrap_or_else(|| format!("view_{}", name.to_lowercase()))
    }
    /// Where it records how far it got. Without this, a restart reprocesses
    /// from the beginning or skips what it did not manage to apply, and both
    /// give a wrong view with no error.
    pub fn checkpoint(name: &str) -> String {
        format!("view_{}_checkpoint", name.to_lowercase())
    }
    /// The rebuild route. It carries no cron —rebuilding is a decision, not a
    /// cadence— but it lives here alongside the other two.
    pub fn rebuild_route(name: &str) -> String {
        format!("/internal/view/{name}/rebuild")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub service: String,
    pub version: Option<String>,
    /// Governance: every service has a human owner and declared criticality.
    pub owner: Option<String>,
    pub tier: Option<String>,
    /// Field names carrying personal data, in any event or method of this
    /// service. The generator uses it to redact logs and `verify` to block
    /// them from leaving through a public route.
    #[serde(default)]
    pub pii: Vec<String>,
    #[serde(default)]
    pub external: bool,
    #[serde(default)]
    pub emits: IndexMap<String, Fields>,
    #[serde(default)]
    pub consumes: IndexMap<String, Consume>,
    #[serde(default)]
    pub methods: IndexMap<String, Method>,
    #[serde(default)]
    pub depends: Vec<Depend>,
    #[serde(default)]
    pub patterns: Patterns,
    /// Side of the CAP theorem. See `Cap`.
    #[serde(default)]
    pub cap: Cap,
    /// The service's feature flags.
    #[serde(default)]
    pub flags: IndexMap<String, Flag>,
    /// Export to the data warehouse.
    #[serde(default)]
    pub analytics: Analytics,
    /// Pooler or sharder in front of the database. See `Pooler`.
    #[serde(default)]
    pub pooler: Pooler,
    /// The domain's state machines: the only business logic worth declaring,
    /// because it is the same in every language.
    #[serde(default)]
    pub machine: IndexMap<String, Machine>,
    /// Sagas this service coordinates. See `Saga`.
    #[serde(default)]
    pub saga: IndexMap<String, Saga>,
    /// Aggregates with event sourcing. See `Aggregate`.
    #[serde(default)]
    pub aggregate: IndexMap<String, Aggregate>,
    /// Read models. See `View`.
    #[serde(default)]
    pub view: IndexMap<String, View>,
    /// Business metrics over the declared events. See `Metric`.
    #[serde(default)]
    pub metrics: IndexMap<String, Metric>,
    #[serde(default)]
    pub infra: Infra,
    /// Per-environment overrides: `[env.prod] min_instances = 3`.
    #[serde(default)]
    pub env: IndexMap<String, Infra>,
    #[serde(skip)]
    pub origin: PathBuf,
}

/// Applies an environment's overrides on top of the base infra. The manifest
/// stays a single one: environments are deltas, not copies.
pub fn for_env(m: &Manifest, env: &str) -> Manifest {
    let mut out = m.clone();
    if let Some(o) = m.env.get(env) {
        if o.state.is_some() {
            out.infra.state = o.state.clone();
        }
        if o.runtime.is_some() {
            out.infra.runtime = o.runtime.clone();
        }
        if o.migrations.is_some() {
            out.infra.migrations = o.migrations.clone();
        }
        if o.min_instances.is_some() {
            out.infra.min_instances = o.min_instances;
        }
        if !o.secrets.is_empty() {
            out.infra.secrets = o.secrets.clone();
        }
        if o.max_instances.is_some() {
            out.infra.max_instances = o.max_instances;
        }
        if o.port.is_some() {
            out.infra.port = o.port;
        }
        if !o.buckets.is_empty() {
            out.infra.buckets = o.buckets.clone();
        }
        if o.tenant_column.is_some() {
            out.infra.tenant_column = o.tenant_column.clone();
        }
        if !o.tenant_exempt.is_empty() {
            out.infra.tenant_exempt = o.tenant_exempt.clone();
        }
        if o.pool_size.is_some() {
            out.infra.pool_size = o.pool_size;
        }
        if o.max_connections.is_some() {
            out.infra.max_connections = o.max_connections;
        }
        if o.read_replicas.is_some() {
            out.infra.read_replicas = o.read_replicas;
        }
        if o.ha.is_some() {
            out.infra.ha = o.ha;
        }
        if o.backup_retention_days.is_some() {
            out.infra.backup_retention_days = o.backup_retention_days;
        }
        if o.pitr.is_some() {
            out.infra.pitr = o.pitr;
        }
    }
    out
}

pub fn load(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut m: Manifest = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    // serde cannot tell "absent" from "equal to the default"; the text can
    m.cap.declared = text.contains("[cap]");
    m.origin = path.to_path_buf();
    Ok(m)
}

/// Merges manifests from disk and from live services. A service publishes its
/// own at /.well-known/axon.json; an external one is frozen into a *.external.toml.
pub fn discover(sources: &[String]) -> Result<Vec<Manifest>, String> {
    let mut out = Vec::new();
    for s in sources {
        if s.starts_with("http://") || s.starts_with("https://") {
            let url = if s.ends_with(".json") {
                s.clone()
            } else {
                format!("{}/.well-known/axon.json", s.trim_end_matches('/'))
            };
            match ureq::get(&url)
                .timeout(std::time::Duration::from_secs(5))
                .call()
            {
                Ok(r) => {
                    let mut m: Manifest = r.into_json().map_err(|e| format!("{url}: {e}"))?;
                    m.origin = PathBuf::from(&url);
                    out.push(m);
                }
                // a service that is down does not break discovery
                Err(e) => eprintln!("axon: {url}: {e}"),
            }
        } else {
            let p = Path::new(s);
            if p.is_dir() {
                let mut files: Vec<_> = std::fs::read_dir(p)
                    .map_err(|e| format!("{s}: {e}"))?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                    // axon.*.toml is axon's own config, not a service
                    .filter(|p| {
                        !p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("axon."))
                    })
                    .collect();
                files.sort();
                for f in files {
                    out.push(load(&f)?);
                }
            } else {
                out.push(load(p)?);
            }
        }
    }
    Ok(out)
}

// ---------- schema derived from the migrations ----------

#[derive(Debug, Clone, Serialize)]
pub struct Column {
    pub name: String,
    pub ty: String,
    pub pk: bool,
    pub fk: Option<String>,
    /// Generates its value from a sequence: `serial`, `bigserial`,
    /// `GENERATED ... AS IDENTITY`. When sharding, each node has its own
    /// sequence and the values collide.
    pub serial: bool,
}

/// A table: its columns and its uniqueness constraints.
///
/// Uniqueness is kept as sets of columns and not as a per-column mark,
/// because that is what decides whether they are safe when sharding: a
/// `UNIQUE (tenant_id, handle)` can be guaranteed by each node on its own;
/// a `UNIQUE (handle)` cannot, and neither can the set of nodes.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Table {
    pub cols: Vec<Column>,
    /// Each entry is the column set of a UNIQUE or PRIMARY KEY constraint.
    pub uniques: Vec<Vec<String>>,
}

impl Table {
    pub fn col(&self, name: &str) -> Option<&Column> {
        self.cols.iter().find(|c| c.name == name)
    }
    pub fn has(&self, name: &str) -> bool {
        self.col(name).is_some()
    }
}

pub type Tables = IndexMap<String, Table>;

use sqlparser::ast::{
    AlterTableOperation, ColumnOption, ObjectType, RenameTableNameKind, Statement, TableConstraint,
};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

/// The schema is read with a real SQL parser. A regex breaks on
/// `PARTITION BY`, composite types, or whatever any ORM generates, and the
/// worst part is that it breaks silently: it returns the wrong columns and
/// nobody notices. This fails loudly, which is the only acceptable thing for
/// the ER diagram and for the cross-service FK check.
fn statements(text: &str, origin: &str) -> Vec<Statement> {
    match Parser::parse_sql(&PostgreSqlDialect {}, text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "axon: {origin}: could not parse the SQL: {e}\n      \
                 axon reads the migrations for the ER diagram and to block FK \
                 between services; better to fail than to guess columns"
            );
            std::process::exit(1);
        }
    }
}

/// Postgres folds every unquoted identifier to lowercase. Keeping the file's
/// casing and quoting it later produces a column that does not exist.
fn ident(i: &sqlparser::ast::Ident) -> String {
    match i.quote_style {
        Some(_) => i.value.clone(),
        None => i.value.to_lowercase(),
    }
}

fn table_name(o: &sqlparser::ast::ObjectName) -> String {
    o.0.last()
        .map(|p| {
            let t = p.to_string();
            match t.starts_with('"') {
                true => t.trim_matches('"').to_string(),
                false => t.to_lowercase(),
            }
        })
        .unwrap_or_default()
}

/// A type has to fit in one token: mermaid's ER is `<type> <column>`.
fn sql_type(t: &sqlparser::ast::DataType) -> String {
    t.to_string().to_lowercase().replace(' ', "_")
}

pub fn parse_ddl(text: &str, origin: &str, into: &mut Tables) {
    for st in statements(text, origin) {
        match st {
            Statement::CreateTable(ct) => {
                let mut uniques: Vec<Vec<String>> = Vec::new();
                let mut cols: Vec<Column> = Vec::new();
                for c in &ct.columns {
                    let n = ident(&c.name);
                    let t = sql_type(&c.data_type);
                    let pk = c
                        .options
                        .iter()
                        .any(|o| matches!(o.option, ColumnOption::PrimaryKey(_)));
                    // column-level UNIQUE and PRIMARY KEY are single-column
                    // uniqueness constraints
                    let uniq = c
                        .options
                        .iter()
                        .any(|o| matches!(o.option, ColumnOption::Unique { .. }));
                    if pk || uniq {
                        uniques.push(vec![n.clone()]);
                    }
                    cols.push(Column {
                        // `serial` and `bigserial` are Postgres sugar for a
                        // sequence, and so is `GENERATED AS IDENTITY`
                        serial: t.contains("serial")
                            || c.options
                                .iter()
                                .any(|o| matches!(o.option, ColumnOption::Generated { .. })),
                        name: n,
                        ty: t,
                        pk,
                        fk: c.options.iter().find_map(|o| match &o.option {
                            ColumnOption::ForeignKey(f) => Some(table_name(&f.foreign_table)),
                            _ => None,
                        }),
                    });
                }
                // the same constraints, declared at table level
                for c in &ct.constraints {
                    match c {
                        TableConstraint::PrimaryKey(pk) => {
                            let cs: Vec<String> = pk
                                .columns
                                .iter()
                                .map(|k| k.to_string().trim_matches('"').to_lowercase())
                                .collect();
                            for k in &cs {
                                mark(&mut cols, k, |c| c.pk = true);
                            }
                            uniques.push(cs);
                        }
                        TableConstraint::ForeignKey(fk) => {
                            let t = table_name(&fk.foreign_table);
                            for k in &fk.columns {
                                let t = t.clone();
                                mark(&mut cols, &k.to_string().to_lowercase(), move |c| {
                                    c.fk = Some(t.clone())
                                });
                            }
                        }
                        TableConstraint::Unique(u) => {
                            uniques.push(
                                u.columns
                                    .iter()
                                    .map(|k| k.to_string().trim_matches('"').to_lowercase())
                                    .collect(),
                            );
                        }
                        _ => {}
                    }
                }
                into.insert(table_name(&ct.name), Table { cols, uniques });
            }
            Statement::AlterTable(at) => {
                let table = table_name(&at.name);
                for op in at.operations {
                    match op {
                        AlterTableOperation::AddColumn { column_def, .. } => {
                            let n = ident(&column_def.name);
                            let t = sql_type(&column_def.data_type);
                            let uniq = column_def
                                .options
                                .iter()
                                .any(|o| matches!(o.option, ColumnOption::Unique { .. }));
                            let entry = into.entry(table.clone()).or_default();
                            if uniq {
                                entry.uniques.push(vec![n.clone()]);
                            }
                            entry.cols.push(Column {
                                serial: t.contains("serial")
                                    || column_def.options.iter().any(|o| {
                                        matches!(o.option, ColumnOption::Generated { .. })
                                    }),
                                name: n,
                                ty: t,
                                pk: false,
                                fk: column_def.options.iter().find_map(|o| match &o.option {
                                    ColumnOption::ForeignKey(f) => {
                                        Some(table_name(&f.foreign_table))
                                    }
                                    _ => None,
                                }),
                            });
                        }
                        // A key added in a LATER migration used to be
                        // invisible: every uniqueness rule —the event
                        // stream's, the sharding ones, a view's checkpoint—
                        // took it as absent and passed in silence.
                        AlterTableOperation::AddConstraint { constraint, .. } => {
                            let entry = into.entry(table.clone()).or_default();
                            let key_cols = |cs: &[sqlparser::ast::IndexColumn]| -> Vec<String> {
                                cs.iter()
                                    .map(|k| k.to_string().trim_matches('"').to_lowercase())
                                    .collect()
                            };
                            match constraint {
                                TableConstraint::PrimaryKey(pk) => {
                                    let cols = key_cols(&pk.columns);
                                    for c in &cols {
                                        mark(&mut entry.cols, c, |x| x.pk = true);
                                    }
                                    entry.uniques.push(cols);
                                }
                                TableConstraint::Unique(u) => {
                                    entry.uniques.push(key_cols(&u.columns));
                                }
                                _ => {}
                            }
                        }
                        // A rename in a LATER migration used to be invisible
                        // too: the schema kept the old name, so every rule
                        // about the renamed table went on checking a table
                        // that no longer exists —and passed, because the old
                        // one still had everything it asked for.
                        AlterTableOperation::RenameTable { table_name: to } => {
                            let to = match &to {
                                RenameTableNameKind::As(n) | RenameTableNameKind::To(n) => {
                                    table_name(n)
                                }
                            };
                            if let Some(i) = into.get_index_of(&table) {
                                let (_, t) = into.swap_remove_index(i).unwrap();
                                into.insert(to, t);
                            }
                        }
                        AlterTableOperation::RenameColumn {
                            old_column_name,
                            new_column_name,
                        } => {
                            let (old, new) = (ident(&old_column_name), ident(&new_column_name));
                            if let Some(tb) = into.get_mut(&table) {
                                if let Some(c) = tb.cols.iter_mut().find(|c| c.name == old) {
                                    c.name = new.clone();
                                }
                                for u in &mut tb.uniques {
                                    for c in u.iter_mut() {
                                        if *c == old {
                                            *c = new.clone();
                                        }
                                    }
                                }
                            }
                        }
                        AlterTableOperation::DropColumn { column_names, .. } => {
                            if let Some(tb) = into.get_mut(&table) {
                                let dropped: Vec<String> = column_names.iter().map(ident).collect();
                                tb.cols.retain(|c| !dropped.contains(&c.name));
                                // uniqueness over a dropped column no longer exists
                                tb.uniques
                                    .retain(|u| !u.iter().any(|c| dropped.contains(c)));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Statement::Drop {
                object_type: ObjectType::Table,
                names,
                ..
            } => {
                for n in names {
                    into.shift_remove(&table_name(&n));
                }
            }
            _ => {}
        }
    }
}

fn mark(cols: &mut [Column], name: &str, f: impl Fn(&mut Column)) {
    let name = name.trim_matches('"');
    if let Some(c) = cols.iter_mut().find(|c| c.name == name) {
        f(c);
    }
}

/// Truly destructive, not "contains the word DROP in a comment".
pub fn destructive(text: &str, origin: &str) -> bool {
    statements(text, origin).iter().any(|st| match st {
        Statement::Drop {
            object_type: ObjectType::Table,
            ..
        } => true,
        Statement::AlterTable(at) => at
            .operations
            .iter()
            .any(|o| matches!(o, AlterTableOperation::DropColumn { .. })),
        _ => false,
    })
}

pub fn migrations_of(m: &Manifest) -> Vec<PathBuf> {
    let Some(path) = &m.infra.migrations else {
        return vec![];
    };
    let p = Path::new(path);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        m.origin.parent().unwrap_or(Path::new(".")).join(p)
    };
    if full.is_dir() {
        let mut files: Vec<_> = std::fs::read_dir(&full)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "sql"))
            .collect();
        files.sort();
        files
    } else if full.exists() {
        vec![full]
    } else {
        vec![]
    }
}

/// The schema IS the sum of the migrations folded in order. There is no
/// duplicated schema.sql to drift.
pub fn schemas(manifests: &[Manifest]) -> IndexMap<String, Tables> {
    let mut out = IndexMap::new();
    for m in manifests {
        let files = migrations_of(m);
        if files.is_empty() {
            continue;
        }
        let mut tables = Tables::new();
        for f in files {
            if let Ok(text) = std::fs::read_to_string(&f) {
                parse_ddl(&text, &f.display().to_string(), &mut tables);
            }
        }
        out.insert(m.service.clone(), tables);
    }
    out
}

// ---------- name helpers ----------

fn words(s: &str) -> Vec<&str> {
    s.split(['.', '@', '_', '-'])
        .filter(|w| !w.is_empty())
        .collect()
}

pub fn pascal(s: &str) -> String {
    words(s).iter().map(|w| upper1(w)).collect()
}

pub fn camel(s: &str) -> String {
    let w = words(s);
    match w.split_first() {
        Some((first, rest)) => {
            first.to_string() + &rest.iter().map(|w| upper1(w)).collect::<String>()
        }
        None => String::new(),
    }
}

fn upper1(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn tfname(s: &str) -> String {
    let out: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    out.split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// Pub/Sub does not allow '@' in topic names.
pub fn topic(ev: &str) -> String {
    ev.replace('@', ".")
}

pub fn ts_type(t: &str) -> &str {
    match t {
        "string" | "timestamp" | "uuid" => "string",
        "int" | "float" => "number",
        "bool" => "boolean",
        "money" => "{ amount: number; currency: string }",
        _ => "unknown",
    }
}

/// Normalizes a field name for comparison: lowercase and no separators.
///
/// The same concept is spelled differently in each layer —`customerEmail` in
/// the contract, `customer_email` in the database, `customer-email` in a
/// header— and declaring it three times in `pii` would be absurd. It is
/// declared once and compared normalized.
pub fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Whether a field is declared as personal, compared normalized.
pub fn is_pii(declared: &[String], field: &str) -> bool {
    let c = normalize(field);
    declared.iter().any(|d| normalize(d) == c)
}

#[cfg(test)]
mod pii {
    use super::*;

    #[test]
    fn the_same_concept_is_declared_once() {
        let d = vec!["customer_email".to_string()];
        for field in [
            "customerEmail",
            "customer_email",
            "CustomerEmail",
            "customer-email",
        ] {
            assert!(is_pii(&d, field), "{field} should match");
        }
        for field in ["customer_id", "email_template", "customer"] {
            assert!(!is_pii(&d, field), "{field} should not match");
        }
    }
}

/// Today's date as (year, month, day), with no dependencies.
///
/// It is Howard Hinnant's civil_from_days: the days since the epoch are
/// shifted to an era starting in March, and there the month pattern is
/// regular. A whole dependency to compare two dates would be too much.
pub fn today() -> (i64, i64, i64) {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 / 86_400)
        .unwrap_or(0);
    civil(days)
}

fn civil(epoch_days: i64) -> (i64, i64, i64) {
    let z = epoch_days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY-MM-DD` to a comparable tuple. `None` if it does not have that shape.
pub fn date(s: &str) -> Option<(i64, i64, i64)> {
    let mut p = s.trim().split('-');
    let a = p.next()?.parse().ok()?;
    let m = p.next()?.parse().ok()?;
    let d = p.next()?.parse().ok()?;
    if p.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((a, m, d))
}

#[cfg(test)]
mod fechas {
    use super::*;

    #[test]
    fn the_civil_calendar_is_correct() {
        // known days since the epoch, leap years and centuries included
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(1), (1970, 1, 2));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(civil(11_016), (2000, 2, 29)); // the leap day of a century divisible by 400
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(20_608), (2026, 6, 4));
    }

    #[test]
    fn today_is_a_reasonable_date() {
        let (a, m, d) = today();
        assert!((2025..2100).contains(&a), "year out of range: {a}");
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&d));
    }

    #[test]
    fn parses_dates() {
        assert_eq!(date("2026-12-31"), Some((2026, 12, 31)));
        assert_eq!(date(" 2026-01-02 "), Some((2026, 1, 2)));
        assert_eq!(date("2026-13-01"), None);
        assert_eq!(date("2026-12"), None);
        assert_eq!(date("manana"), None);
    }
}
