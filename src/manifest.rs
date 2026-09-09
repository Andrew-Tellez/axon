//! Loading, discovery, and the schema derived from the migrations.
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub type Fields = IndexMap<String, String>;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Consume {
    pub handler: String,
    /// The fields of the event this service actually READS.
    ///
    /// Absent means nobody declared it and nothing can be concluded. An EMPTY
    /// list is a declaration too: it reads none of them, which is the honest
    /// answer for a handler that only reacts.
    ///
    /// Declaring it is what turns "somebody might be using this" into a
    /// question with an answer. It is not documentation either: the generated
    /// handler receives `Pick<Event, ...>`, so reading a field that was not
    /// declared does not compile, and the declaration cannot drift from the
    /// code the way a Pact recorded once does.
    pub uses: Option<Vec<String>>,
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
    /// What the caller has to present to call this, beyond being someone.
    ///
    /// `auth = "required"` says the caller is authenticated and nothing else:
    /// any valid token can refund a payment. A scope is the difference between
    /// "somebody" and "somebody allowed to do this", and it is declared here
    /// because the alternative is an `if` in a handler that nobody can audit
    /// from outside.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Who may call it, by role. The requirement is the contract and goes in
    /// the OpenAPI and in the generated guard; the mapping from a role to its
    /// scopes is the provider's and is deliberately not declared here.
    #[serde(default)]
    pub roles: Vec<String>,
    /// The entitlement it takes: a plan, a tier, a contracted feature. Same
    /// shape as `roles` and for the same reason.
    #[serde(default)]
    pub plans: Vec<String>,
    /// Requests per minute at the gateway.
    pub rate_limit: Option<u32>,
    /// Time budget at the edge.
    pub timeout_ms: Option<u32>,
    /// Returns a collection: forces cursor pagination.
    #[serde(default)]
    pub paginated: bool,
    /// How it fails. Declared like `in` and `out`, and for the same reason: the
    /// failures of a method are part of its contract, and today they live in the
    /// handler's body —where the caller cannot see them— so every caller invents
    /// its own reading of a 500.
    ///
    /// What this buys is not documentation: `retriable` CHANGES the generated
    /// client. Retrying a declined card is nonsense that burns the caller's time
    /// budget and ends in the same answer, so a declared non-retriable failure is
    /// not retried at all.
    #[serde(default)]
    pub errors: Vec<Failure>,
    /// The date it stopped being the version to use, in YYYY-MM-DD. Its presence
    /// is what makes the method deprecated.
    ///
    /// A version is not retired by announcing it in a chat: it is retired when
    /// every caller stops calling it, and for that they have to find out from
    /// the same place they call. Declared here it travels in the response —RFC
    /// 9745— so a client sees it in its own logs, and `verify` can name who is
    /// still calling it.
    pub deprecated: Option<String>,
    /// The date it stops being served, in YYYY-MM-DD. RFC 8594.
    ///
    /// A deprecation with no date does not end: v1 stays up for years because
    /// nobody can point at the day it dies.
    pub sunset: Option<String>,
    /// The method that replaces it. It becomes a `Link rel="successor-version"`,
    /// because telling somebody that what they use is dying without saying what
    /// to use instead moves the problem, it does not solve it.
    pub successor: Option<String>,
    /// The shape of this method as of an older version, keyed by version.
    ///
    /// Only what CHANGED gets declared: a version with no entry here did not
    /// change this method, and the adapter chain skips it.
    #[serde(default)]
    pub at: IndexMap<String, Shape>,
}

impl Method {
    /// Whether it carries a declared retirement of any kind.
    pub fn retiring(&self) -> bool {
        self.deprecated.is_some() || self.sunset.is_some()
    }
}

/// A declared failure of a method.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Failure {
    /// The code the caller matches on, in `snake_case`. It travels in the
    /// `problem+json` body, so it is as much a contract as a field name: two
    /// services reading `card_declined` differently is the same class of problem
    /// as two reading `total` differently.
    pub code: String,
    /// The HTTP status it comes out as at the edge. A failure is not a 2xx, and
    /// a caller that has to parse prose to tell "declined" from "provider down"
    /// has no contract at all.
    pub status: u16,
    /// Whether trying again can end differently. `false` is the honest default:
    /// most failures are decisions, not weather.
    #[serde(default)]
    pub retriable: bool,
    /// One line for the OpenAPI, because a code with no sentence is a code
    /// somebody will guess at.
    pub detail: Option<String>,
}

/// The statuses where retrying can end differently, among the 4xx. The rest of
/// the 4xx are decisions about the request: the same request gets the same
/// answer, and a retry only burns the caller's budget.
pub const RETRIABLE_4XX: [u16; 3] = [408, 425, 429];

impl Failure {
    /// Whether the code is in the shape that survives crossing a service
    /// boundary: `snake_case`, no accents, no spaces.
    pub fn well_formed(&self) -> bool {
        !self.code.is_empty()
            && self
                .code
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }
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
    /// The fields of the answer this service actually READS. The generated
    /// client returns `Pick<..., uses>`: what is not declared does not compile.
    ///
    /// Absent means undeclared; empty means it reads none of them, which is
    /// what a compensation whose answer nobody looks at really does.
    pub uses: Option<Vec<String>>,
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

/// The API's versioning.
///
/// `"path"` —the default— puts the version in the route: `/v1/...` and `/v2/...`
/// are two methods that coexist, and the old one declares its retirement.
///
/// `"header"` is what Stripe does: the route never changes, the caller pins a
/// dated version, and the server keeps ONE implementation —the current one—
/// plus an adapter per version that changed a shape. That is what lets a
/// version from years ago stay alive: nobody maintains N implementations, they
/// maintain N small adapters.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Api {
    /// `"path"` or `"header"`. Absent means `"path"`, which is what a service
    /// that never thought about this already does.
    pub versioning: Option<String>,
    /// The header the caller pins with. Defaults to `X-Api-Version`.
    pub header: Option<String>,
    /// What an unpinned caller gets. Normally the newest.
    pub default: Option<String>,
    /// The maintenance cycle, in days: nothing gets retired before this long
    /// after it shipped. It is the promise a platform makes, and the only way
    /// to make it refutable — without it, "we support it for a year" lives in
    /// a blog post and dies in a sprint.
    pub support_window_days: Option<i64>,
    /// The same, for a version marked `lts`. If an LTS does not live longer
    /// than a normal one, the label says nothing.
    pub lts_window_days: Option<i64>,
    /// Every scope that exists on this platform.
    ///
    /// A catalogue and not free text: a scope with a typo is a 403 in
    /// production that nobody sees in a review, and the only way to catch it
    /// beforehand is for the list of what exists to be somewhere.
    pub scopes: Vec<String>,
    /// The dated versions, oldest first, as `[[api.version]]` entries.
    ///
    /// They are entries and not bare strings because a version carries more
    /// than its date: whether it is long-term support and when it dies. A list
    /// of strings cannot say either, and both are what a caller plans around.
    #[serde(rename = "version")]
    pub versions: Vec<Version>,
}

/// One dated version of the API.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Version {
    /// The day it shipped, and its id: the caller pins this string.
    pub date: String,
    /// The day it stopped being the one to use. A version older than the
    /// current one is not automatically deprecated: it can be supported for
    /// years, and that is the difference this field carries.
    pub deprecated: Option<String>,
    /// Long-term support: it lives longer than the ones around it, which is a
    /// promise somebody makes to whoever integrates.
    #[serde(default)]
    pub lts: bool,
    /// When it stops being served. Required on an LTS: "long term" with no date
    /// is not a promise, it is a hope, and it is what leaves a version from
    /// years ago alive because nobody can point at the day it dies.
    pub sunset: Option<String>,
}

impl Version {
    /// Where it is in the maintenance cycle, given today and which one is
    /// current. Derived, not declared: a version stops being current the day a
    /// newer one ships, and nobody should have to remember to write that down.
    pub fn stage(&self, today: (i64, i64, i64), current: &str) -> &'static str {
        let past = |d: &Option<String>| d.as_deref().and_then(date).is_some_and(|d| d <= today);
        if past(&self.sunset) {
            "retired"
        } else if self.date == current {
            "current"
        } else if past(&self.deprecated) {
            "deprecated"
        } else if self.lts {
            "lts"
        } else {
            "supported"
        }
    }
}

impl Api {
    pub fn by_header(&self) -> bool {
        self.versioning.as_deref() == Some("header")
    }
    pub fn header_name(&self) -> &str {
        self.header.as_deref().unwrap_or("X-Api-Version")
    }
    /// The newest declared version: what a new integration should get.
    pub fn newest(&self) -> Option<&Version> {
        self.versions.last()
    }
    /// What an unpinned caller gets.
    pub fn current(&self) -> Option<&str> {
        self.default
            .as_deref()
            .or_else(|| self.newest().map(|v| v.date.as_str()))
    }
    pub fn dates(&self) -> Vec<&str> {
        self.versions.iter().map(|v| v.date.as_str()).collect()
    }
    pub fn find(&self, date: &str) -> Option<&Version> {
        self.versions.iter().find(|v| v.date == date)
    }
}

/// A rule over a metric: the condition, and what it proposes.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Rule {
    /// The declared metric it watches.
    pub metric: String,
    /// The segment the rule applies to: one value per dimension of the metric.
    ///
    /// A metric grouped by something is not one series, it is one per group.
    /// Without saying which, a rule would be comparing a number against another
    /// one from a different group. And it is what makes "this rule is only for
    /// this kind of customer" declarable: the segment has to be a DIMENSION
    /// that the event carries, not a list of ids kept somewhere else — a
    /// contract cannot verify a cohort it does not own.
    #[serde(rename = "where", default)]
    pub segment: IndexMap<String, String>,
    /// What it compares the current window against.
    ///
    /// `previous` is the window right before, `same_day_last_week` is seven
    /// days back —which is what takes the weekly shape out of the comparison—
    /// and `absolute` compares against `value`.
    pub compare: Option<String>,
    /// The ratio it has to fall below, or rise above, against the reference.
    pub below: Option<f64>,
    pub above: Option<f64>,
    /// The number to compare against when `compare = "absolute"`.
    pub value: Option<f64>,
    /// Consecutive windows the condition has to hold. One is a bad hour, and
    /// acting on a bad hour is how a rule earns being turned off.
    #[serde(rename = "for", default = "one")]
    pub sustained: u32,
    /// Windows that must have been quiet before it proposes again.
    ///
    /// It is data and not state: what it says is that the condition was NOT
    /// holding in that many windows before it started to, so a rule proposes on
    /// the way IN and not once per window while it lasts. That way there is
    /// nothing to remember and nothing to get out of sync.
    pub cooldown: Option<u32>,
    /// Metrics that have to hold for it to propose at all.
    ///
    /// A rule with one metric and a lever is Goodhart's law with a cron: the
    /// lever moves the number it is judged by, and nobody is watching what it
    /// moves in the other direction. A guard is the other direction, declared.
    #[serde(default, rename = "guard")]
    pub guards: Vec<Guard>,
    /// `propose` is the only mode there is. `apply` gets refused with its
    /// reason: writing to production off a metric is a control loop, and that
    /// is a decision to take on its own and not a value in a field.
    pub mode: Option<String>,
    /// What it proposes.
    #[serde(default)]
    pub then: Then,
}

/// A metric that has to hold for the rule to propose.
///
/// Same shape as the condition of the rule and without its decision: a guard
/// does not have `for` nor `cooldown`, it is read at the SAME windows the
/// trigger held. And a guard whose data does not come back is not a guard, so
/// the rule does not propose either.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Guard {
    pub metric: String,
    #[serde(rename = "where", default)]
    pub segment: IndexMap<String, String>,
    pub compare: Option<String>,
    pub below: Option<f64>,
    pub above: Option<f64>,
    pub value: Option<f64>,
}

impl Guard {
    pub fn comparison(&self) -> &str {
        self.compare.as_deref().unwrap_or("previous")
    }
    pub fn back(&self) -> u32 {
        match self.comparison() {
            "same_day_last_week" => 7,
            _ => 1,
        }
    }
    /// The series' name in the emitted query: the rule reads several metrics,
    /// so each row has to say which one it is.
    pub fn series(&self) -> String {
        format!("guard:{}", self.metric)
    }
    pub fn label(&self) -> String {
        let dir = match (self.below, self.above) {
            (Some(b), _) => format!("below {b}"),
            (_, Some(a)) => format!("above {a}"),
            _ => "no threshold".to_string(),
        };
        format!("`{}` {dir} vs {}", self.metric, self.comparison())
    }
}

/// What a rule proposes: exactly one of the three.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Then {
    /// A declared flag, set to one of its declared variants.
    pub flag: Option<String>,
    pub variant: Option<String>,
    /// The variant to go back to when the condition stops holding. What goes up
    /// on its own has to be able to come back down on its own.
    pub restore: Option<String>,
    /// An event of this service, so whoever wants to react can.
    pub emits: Option<String>,
    /// A method of this service.
    pub calls: Option<String>,
}

pub const COMPARISONS: [&str; 3] = ["previous", "same_day_last_week", "absolute"];

impl Rule {
    pub fn comparison(&self) -> &str {
        self.compare.as_deref().unwrap_or("previous")
    }
    /// How many windows back the reference is.
    pub fn back(&self) -> u32 {
        match self.comparison() {
            "same_day_last_week" => 7,
            _ => 1,
        }
    }
    /// The segment, written out, for whoever reads the proposal.
    pub fn segment_label(&self) -> String {
        if self.segment.is_empty() {
            "every row".to_string()
        } else {
            self.segment
                .iter()
                .map(|(k, v)| format!("{k} = {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }
    pub fn actions(&self) -> usize {
        [
            self.then.flag.is_some(),
            self.then.emits.is_some(),
            self.then.calls.is_some(),
        ]
        .iter()
        .filter(|x| **x)
        .count()
    }
}

/// The shape of a method as of an older version, and who adapts it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Shape {
    #[serde(rename = "in", default)]
    pub input: Fields,
    #[serde(rename = "out", default)]
    pub output: Fields,
    /// The adapter that translates between this shape and the current one. The
    /// field mapping is business logic and a person writes it; what axon
    /// generates is the typed obligation and the chain that applies it.
    pub adapter: Option<String>,
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
    /// How long the events of this service are kept, in days.
    ///
    /// A table of events grows forever, and the first symptom is the bill while
    /// the second is a query that times out. It goes here and not in each
    /// event because it is a decision about the SERVICE's data —how long the
    /// business needs to look back— with an exception per event for the ones
    /// somebody has to answer for by law.
    pub retention_days: Option<i64>,
    /// The exceptions, by event: `"order.placed@v1" = 2555`, seven years,
    /// because a tax authority says so and not because a dashboard wants it.
    #[serde(default)]
    pub retention: IndexMap<String, i64>,
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
            retention_days: None,
            retention: IndexMap::new(),
            warehouse: "bigquery".into(),
        }
    }
}

impl Analytics {
    /// How long one event is kept: its own exception, or the service's.
    pub fn keeps(&self, event: &str) -> Option<i64> {
        self.retention.get(event).copied().or(self.retention_days)
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

/// How a service runs. `container` listens on a port; `job` runs and ends.
///
/// It is not decoration: a job has no instances to scale, serves no routes and
/// is not behind the edge. Declaring a CLI or a nightly process as a container
/// meant the only honest answer was to declare nothing at all, and then the
/// infrastructure of the thing that actually runs your business lived in
/// somebody's crontab.
pub const RUNTIMES: [&str; 2] = ["container", "job"];

// A key nobody reads is the silent failure this project exists to catch, and
// TOML makes it easy: a top-level key written after a table belongs to that
// table. `include = [...]` under `[infra]` parsed fine and did nothing.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Infra {
    pub state: Option<String>,
    pub runtime: Option<String>,
    /// When it runs, for `runtime = "job"`: a five-field cron expression.
    ///
    /// Absent means somebody triggers it —a backfill, a CLI—, and that is a
    /// legitimate thing to declare: it says the service is NOT a process
    /// listening on a port, which is what decides everything downstream.
    pub schedule: Option<String>,
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

impl Infra {
    /// Runs and ends, instead of listening on a port.
    pub fn is_job(&self) -> bool {
        self.runtime.as_deref() == Some("job")
    }
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

/// A search index.
///
/// The same shape as a cache and for the same reason: it is a **derived copy**,
/// and the only hard part is knowing when it stopped matching. The events are
/// declared, so what has to reindex it is derivable instead of remembered.
///
/// What makes it worse than a cache, and why the rules are stricter: a stale
/// cache serves one wrong answer to whoever asked for that key. A stale or
/// unfiltered index LISTS rows — somebody else's rows, or rows that no longer
/// exist — and the caller never asked for them by name, so nothing about the
/// answer looks wrong.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Indexed {
    /// The table the documents come from. It has to exist in the migrations.
    pub of: String,
    /// The document's id, in the name the events use. It is what a reindex
    /// says changed, so it has to be a field the event carries.
    pub key: String,
    /// The column that id maps to, when the table calls it something else.
    /// `id` for a `key = "itemId"`, the same way a `[crud.*]` does it.
    pub key_column: Option<String>,
    /// What is searchable. Every one has to be a column.
    #[serde(default)]
    pub fields: Vec<String>,
    /// What every query MUST filter by. In a multi-tenant service the tenant
    /// goes here or the search returns other people's rows.
    #[serde(default)]
    pub filter_by: Vec<String>,
    /// The events after which a document is no longer what it says.
    #[serde(default)]
    pub reindexed_by: Vec<String>,
    /// Personal data that goes into the index ON PURPOSE.
    ///
    /// Support looking a customer up by e-mail is a real need, and an index is
    /// a second copy of that data outside the database, usually unencrypted
    /// and often replicated. So it is not forbidden and it is not silent: the
    /// field is named here, the way `tenant_exempt` names a table.
    #[serde(default)]
    pub pii_indexed: Vec<String>,
}

/// The search engine. One per service, like the database and the cache.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Search {
    /// `meilisearch` is native. Absent means no search.
    pub engine: Option<String>,
    #[serde(flatten)]
    pub indexes: IndexMap<String, Indexed>,
}

impl Search {
    pub fn active(&self) -> bool {
        self.engine.as_deref().is_some_and(|e| e != "none")
    }
}

impl Indexed {
    pub fn key_column(&self) -> String {
        self.key_column.clone().unwrap_or_else(|| self.key.clone())
    }
}

pub const SEARCH_ENGINES: [&str; 2] = ["meilisearch", "none"];

/// A CRUD, declared once.
///
/// The five endpoints that have no business logic and that every service
/// rewrites anyway: create, read, update, delete, list. Written by hand they
/// are five routes, five scopes, five entries in the OpenAPI, five handlers
/// and five chances to forget the tenant in the `WHERE`.
///
/// Declared, they expand into ordinary `[methods.*]` before anything else
/// looks at the manifest, so `verify`, the OpenAPI, the testkit, the edge and
/// the generated client work on them with no new machinery. And because the
/// compiler already reads the migrations with a real SQL parser, a CRUD over a
/// column that does not exist fails at build instead of at the first request.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Crud {
    /// The table it is over. It has to exist in the declared migrations.
    pub table: String,
    /// The key, as the caller names it: `itemId` for a column `id`.
    pub key: String,
    /// The column that key maps to. `id` by default.
    pub key_column: Option<String>,
    /// The writable shape. Every one has to be a column of the table.
    #[serde(default)]
    pub fields: Fields,
    /// The route prefix: `/v1/items`.
    pub path: String,
    /// What each half demands. Reads and writes are not the same permission,
    /// and giving them one scope is how a token issued to read deletes a row.
    pub read_scope: Option<String>,
    pub write_scope: Option<String>,
    /// Who may write. Same shape as a method's `roles`.
    #[serde(default)]
    pub write_roles: Vec<String>,
    /// Which of the five to generate. All of them by default: a CRUD missing
    /// its delete is a decision, and it is written down.
    #[serde(default = "crud_all")]
    pub operations: Vec<String>,
}

fn crud_all() -> Vec<String> {
    CRUD_OPERATIONS.iter().map(|s| s.to_string()).collect()
}

pub const CRUD_OPERATIONS: [&str; 5] = ["create", "read", "update", "delete", "list"];

impl Crud {
    pub fn key_column(&self) -> String {
        self.key_column.clone().unwrap_or_else(|| "id".into())
    }
    pub fn has(&self, op: &str) -> bool {
        self.operations.iter().any(|o| o == op)
    }
}

/// Expands every `[crud.*]` into the methods it stands for.
///
/// It runs at load, before anything reads the manifest, so nothing downstream
/// has to know that a CRUD exists: what it sees is five declared methods, and
/// every rule that already applies to a method applies to these.
pub fn expand(m: &mut Manifest) -> Result<(), String> {
    let cruds: Vec<(String, Crud)> = m.crud.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (name, c) in cruds {
        let entity = pascal(&name);
        let tenant = m.infra.tenant_column.clone();
        // The tenant travels in the ROUTE, like everywhere else in axon: it is
        // what lets the sharder route the query and the RLS bind it, and a
        // CRUD that leaves it implicit is the one place it gets forgotten.
        let (prefix, tenant_in) = match &tenant {
            Some(col) => (
                format!(
                    "/v1/tenants/{{{}}}{}",
                    camel(col),
                    c.path.trim_start_matches("/v1")
                ),
                Some(camel(col)),
            ),
            None => (c.path.clone(), None),
        };
        let with_key = |mut f: Fields| -> Fields {
            if let Some(t) = &tenant_in {
                f.shift_insert(0, t.clone(), "uuid".to_string());
            }
            f
        };
        let mut out_fields = c.fields.clone();
        out_fields.shift_insert(0, c.key.clone(), "uuid".to_string());

        let mut add =
            |op: &str, verb: &str, path: String, input: Fields, output: Fields, idem: bool| {
                if !c.has(op) {
                    return;
                }
                let method = format!("{op}{entity}");
                // A method declared by hand WINS, whole. That is the override, and
                // it is one concept and not a second dialect: `axon crud --expand`
                // prints what would be generated, ready to paste and edit. A
                // partial override —"the same but with another `out`"— would be a
                // second language with its own merge rules, and the day the two
                // disagree nobody knows which one is serving.
                if m.methods.contains_key(&method) {
                    return;
                }
                let write = matches!(op, "create" | "update" | "delete");
                m.methods.insert(
                    method,
                    Method {
                        input,
                        output,
                        http: Some(format!("{verb} {path}")),
                        idempotent: idem,
                        auth: Some("required".into()),
                        scopes: match write {
                            true => c.write_scope.clone().into_iter().collect(),
                            false => c.read_scope.clone().into_iter().collect(),
                        },
                        roles: match write {
                            true => c.write_roles.clone(),
                            false => Vec::new(),
                        },
                        paginated: op == "list",
                        ..Default::default()
                    },
                );
            };
        let key_path = format!("{prefix}/{{{}}}", c.key);
        let mut key_in = Fields::new();
        key_in.insert(c.key.clone(), "uuid".to_string());
        // The create takes the KEY from the caller, and is therefore
        // idempotent. axon's own rule refuses a mutation that is not —a client
        // retry would duplicate the row— and a server-generated id is exactly
        // what makes that impossible to fix. A uuid from the caller costs
        // nothing and makes the retry safe.
        let create_in = {
            let mut f = c.fields.clone();
            f.shift_insert(0, c.key.clone(), "uuid".to_string());
            with_key(f)
        };
        add(
            "create",
            "POST",
            prefix.clone(),
            create_in,
            {
                let mut f = Fields::new();
                f.insert(c.key.clone(), "uuid".to_string());
                f
            },
            true,
        );
        add(
            "read",
            "GET",
            key_path.clone(),
            with_key(key_in.clone()),
            out_fields.clone(),
            false,
        );
        add(
            "update",
            "PATCH",
            key_path.clone(),
            {
                let mut f = key_in.clone();
                for (k, v) in &c.fields {
                    f.insert(k.clone(), v.clone());
                }
                with_key(f)
            },
            out_fields.clone(),
            true,
        );
        add(
            "delete",
            "DELETE",
            key_path,
            with_key(key_in),
            {
                let mut f = Fields::new();
                f.insert("deleted".into(), "bool".into());
                f
            },
            true,
        );
        // The list pages by CURSOR and not by offset, because axon's own rule
        // over `paginated` says so: an offset breaks as the table grows, and a
        // generated endpoint has no excuse to be the one that ignores it.
        let mut list_in = Fields::new();
        list_in.insert("cursor".into(), "string".into());
        list_in.insert("limit".into(), "int".into());
        let mut list_out = Fields::new();
        list_out.insert("items".into(), "json".into());
        list_out.insert("cursor".into(), "string".into());
        add("list", "GET", prefix, with_key(list_in), list_out, false);
    }
    Ok(())
}

/// How a caller becomes a principal.
///
/// axon authenticates nobody and holds no credential. What is declared here is
/// the SHAPE a verified token must have, so that the `auth = "required"` at
/// the edge, the `scopes` of a method and the RLS the compiler already
/// generates stop being three independent hopes that happen to agree.
///
/// Who mints the token —better-auth, Auth0, Keycloak, Cognito, thirty lines of
/// jose— is an adapter, exactly like `Bus`, `Cache` and `Outbox`. No provider
/// is named in this block, and that absence is the point: a field that only
/// makes sense for one vendor does not belong to a compiler. The claim names
/// are the whole provider-specific surface, and they are data.
///
/// What is deliberately NOT here: the mapping from a role to its scopes. That
/// is the provider's mutable configuration, which axon can neither observe nor
/// diff, so declaring it would be a second copy that goes stale in silence —
/// and a rule over it would report as an error what somebody correctly changed
/// on the other side.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Auth {
    /// Accepted issuers. A list and not one value because the longest-lived
    /// event in an auth system's life is a migration: two issuers accepted for
    /// six weeks. With a single field that window is a red CI or a lie.
    #[serde(default)]
    pub issuers: Vec<String>,
    /// This service's audience. Per service, unlike the rest: a token minted
    /// for the reporting service accepted by the payments service is a total,
    /// invisible failure —every signature checks out and every log line looks
    /// normal.
    pub audience: Option<String>,
    /// `jwks` verifies offline, `introspection` asks the issuer, `adapter`
    /// means axon promises nothing and only hands over the interface. It is
    /// the field that decides what the compiler may state about revocation.
    pub verify: Option<String>,
    pub jwks_uri: Option<String>,
    pub introspection_url: Option<String>,
    /// A closed list, not a preference. `none` accepted anywhere makes every
    /// forged token valid, and an HS* alongside a published key set lets
    /// somebody sign with the public key as if it were the secret. Both fail
    /// OPEN, and both look like a normal 200.
    #[serde(default)]
    pub algorithms: Vec<String>,
    pub clock_skew_s: Option<u32>,
    /// The worst case between a revocation and its effect. Without a number,
    /// "revoked" is a word the compiler cannot print anywhere.
    pub max_token_age_s: Option<u32>,
    /// `eventual` or `immediate`. `immediate` over offline verification is a
    /// promise the mechanism cannot keep.
    pub revocation: Option<String>,
    /// Where each thing axon already reasons about lives inside the token.
    pub subject_claim: Option<String>,
    /// Mandatory when `[infra] tenant_column` is set: it is what binds the RLS
    /// to the caller instead of to whatever the handler happened to pass.
    pub tenant_claim: Option<String>,
    pub scopes_claim: Option<String>,
    pub roles_claim: Option<String>,
    /// Acting on somebody else's behalf. See `Impersonation`.
    pub impersonation: Option<Impersonation>,
}

/// Support acting as a user, and the two questions that decides.
///
/// Which tenant the RLS binds to —the impersonated one, or the row is invisible
/// and the ticket unanswerable— and whether the write says who really did it.
/// A change made in somebody else's name with no trace is the worst version of
/// this, so the audit is not optional.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Impersonation {
    /// The claim carrying the real actor. RFC 8693 calls it `act`.
    pub claim: String,
    /// Who may impersonate. Names, checked against the declared catalogue: a
    /// typo here is a grant that silently never applies.
    #[serde(default)]
    pub roles: Vec<String>,
    /// Every write stamped with the actor. `false` has to be written by hand,
    /// and `verify` says what it costs.
    #[serde(default)]
    pub audit: bool,
}

pub const AUTH_VERIFY: [&str; 3] = ["jwks", "introspection", "adapter"];
pub const AUTH_REVOCATION: [&str; 2] = ["eventual", "immediate"];
/// Refused outright: `none` is no verification, and an HMAC over a published
/// key set is a shared secret wearing a public key's clothes.
pub const WEAK_ALGORITHMS: [&str; 4] = ["none", "HS256", "HS384", "HS512"];

/// A catalog: a small, fixed list that several services have to agree on.
///
/// Currencies, statuses, reasons, countries. The list nobody thinks is worth
/// declaring, so it ends up written three times —an enum in one service, a
/// `CHECK` in a migration, a dropdown in the front— and the day somebody adds
/// a value, two of the three do not hear about it.
///
/// Declared, the list is ONE: the table, its seed, and a union type that makes
/// an invented value not compile. And because it is in the manifest, adding a
/// value is a diff somebody reviews instead of an `INSERT` somebody ran.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    /// The field that identifies each entry. It has to be one of `fields`, and
    /// it is what the rest of the schema points at.
    pub key: String,
    /// The columns, with the same types as everywhere else in the manifest.
    #[serde(default)]
    pub fields: Fields,
    /// The rows. In the manifest on purpose: a catalog whose values live only
    /// in the database is a catalog nobody can review, and the code cannot
    /// know them either.
    #[serde(default)]
    pub entries: Vec<IndexMap<String, toml::Value>>,
    /// The table. `catalog_<name>` by default.
    pub table: Option<String>,
}

impl Catalog {
    pub fn table(&self, name: &str) -> String {
        self.table
            .clone()
            .unwrap_or_else(|| format!("catalog_{}", name.to_lowercase()))
    }
}

/// A cached answer.
///
/// A cache is not another storage engine: it is a **derived copy**, and the
/// only hard part is knowing when it stopped being true. That is the one thing
/// axon knows and a library cannot: the events are declared, so what makes a
/// cached answer stale is derivable instead of remembered.
///
/// Everything here exists because of a failure that has no symptom. A cache
/// with nothing to invalidate it serves the wrong answer forever and the
/// dashboards stay green. A key without the tenant serves one customer's data
/// to another and looks like a hit. A TTL longer than the declared staleness
/// budget makes `[cap]` a lie.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cached {
    /// The method whose answer is cached. It has to be declared, and it has to
    /// be a read: caching a write is not a cache.
    pub of: String,
    /// The fields of that method's `in` the key is built from. They have to be
    /// its own: a key made of something the method does not receive is a key
    /// two different requests can share.
    #[serde(default)]
    pub key: Vec<String>,
    /// How long a hit stays valid. Bounded by `[cap] max_staleness_ms`,
    /// because that number is the promise and this one is what keeps it.
    pub ttl_ms: Option<u32>,
    /// The events after which the answer is no longer true.
    ///
    /// This service has to be able to SEE them —emit them or consume them— or
    /// it cannot invalidate anything, and an invalidation nobody can run is
    /// worse than none: it reads as handled.
    #[serde(default)]
    pub invalidated_by: Vec<String>,
    /// What happens when one of those events arrives.
    ///
    /// `invalidate` drops the entry and the next reader pays for the reload.
    /// `refresh` REWRITES it from the event itself, with no reload at all —
    /// which is only possible if the event carries every field of the answer,
    /// and that is checkable: `verify` names the field that is missing instead
    /// of letting the cache fill with holes.
    pub strategy: Option<String>,
    /// Serve the expired answer while one reader refreshes it, up to this
    /// long. It is what turns a stampede into one reload, and it SPENDS
    /// staleness: `ttl_ms + stale_ms` is what has to fit the declared budget.
    pub stale_ms: Option<u32>,
    /// The declared flag that turns it on. The day the invalidation is wrong,
    /// what you want is a switch you can flip without a deploy — and once the
    /// cache is behind a flag, a `[rules.*]` over a metric can flip it: that
    /// loop already exists and this is what plugs the cache into it.
    ///
    /// The flag has to be boolean and, if it is pinned by a field, the cached
    /// method has to receive that field: a decision pinned to something the
    /// method never sees is a decision taken per request.
    pub enabled_by: Option<String>,
    /// One reader reloads and the rest wait for it. `true` by default because
    /// the alternative —every instance missing at once and hitting the
    /// database together— is how a cache takes down what it was protecting.
    #[serde(default = "yes")]
    pub single_flight: bool,
}

fn yes() -> bool {
    true
}

pub const CACHE_STRATEGIES: [&str; 2] = ["invalidate", "refresh"];

/// The cache engine. One per service, like the database: a shared cache is a
/// shared blast radius and a shared key namespace.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Cache {
    /// `valkey` is native. Absent means no cache.
    pub engine: Option<String>,
    /// The entries: `[cache.<name>]`.
    #[serde(flatten)]
    pub entries: IndexMap<String, Cached>,
}

impl Cache {
    pub fn active(&self) -> bool {
        self.engine.as_deref().is_some_and(|e| e != "none")
    }
}

pub const CACHE_ENGINES: [&str; 2] = ["valkey", "none"];

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
    /// How the API is versioned. A platform decision, and `verify` requires
    /// every service to declare the same one: two schemes at once means the
    /// caller has to know which service it is talking to before it can know
    /// how to ask for a version.
    #[serde(default)]
    pub api: Api,
    /// Files this manifest is split into, relative to its own directory.
    ///
    /// A service grows and its manifest with it. The blocks of a feature —its
    /// methods, its events, its machine— can live in their own file, and each
    /// entry here is a file or a DIRECTORY, in which case every `*.toml` inside
    /// it counts, sorted, so adding a feature is adding a file.
    ///
    /// What does NOT get split is the service's own: `[infra]`, `[cap]`,
    /// `[analytics]`, `[api]`, `[patterns]`, `[pooler]` and `[env.*]` belong to
    /// the service and not to one of its features, and a fragment declaring one
    /// is refused instead of merged.
    ///
    /// It is not serialized: what a service serves and what the baseline
    /// records is the MERGED manifest, and the layout of the source is not part
    /// of the contract. Splitting a file has to be invisible from the outside
    /// or it is not a split, it is a change.
    #[serde(default, skip_serializing)]
    pub include: Vec<String>,
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
    /// Cached answers, with what makes them stale. See `Cache`.
    #[serde(default)]
    pub cache: Cache,
    /// Fixed lists several services have to agree on. See `Catalog`.
    #[serde(default)]
    pub catalog: IndexMap<String, Catalog>,
    /// The shape a verified token must have. See `Auth`.
    #[serde(default)]
    pub auth: Auth,
    /// The five endpoints with no business logic. See `Crud`.
    #[serde(default, skip_serializing)]
    pub crud: IndexMap<String, Crud>,
    /// Search indexes, with what reindexes them. See `Search`.
    #[serde(default)]
    pub search: Search,
    /// Business metrics over the declared events. See `Metric`.
    #[serde(default)]
    pub metrics: IndexMap<String, Metric>,
    /// Rules that watch a declared metric and say what to do when it moves.
    ///
    /// The loop that today lives in a dashboard alert plus a runbook nobody
    /// ran: the metric is already declared, the flag is already declared, and
    /// the event catalogue too, so what was missing was saying out loud which
    /// condition on which metric leads to which of them.
    ///
    /// It only ever PROPOSES. Nothing here writes to production: the value is
    /// that the decision stops being oral, and a control loop over production
    /// is a different decision that has to be taken on its own.
    #[serde(default)]
    pub rules: IndexMap<String, Rule>,
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

/// A piece of a service's manifest: the blocks of one feature.
///
/// Only what belongs to a feature. The rest is the service's, and mixing the
/// two is how a `[cap]` ends up declared twice with different answers.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Fragment {
    pub pii: Vec<String>,
    pub emits: IndexMap<String, Fields>,
    pub consumes: IndexMap<String, Consume>,
    pub methods: IndexMap<String, Method>,
    pub depends: Vec<Depend>,
    pub flags: IndexMap<String, Flag>,
    pub machine: IndexMap<String, Machine>,
    pub saga: IndexMap<String, Saga>,
    pub aggregate: IndexMap<String, Aggregate>,
    pub view: IndexMap<String, View>,
    pub metrics: IndexMap<String, Metric>,
    pub catalog: IndexMap<String, Catalog>,
    pub crud: IndexMap<String, Crud>,
}

/// The blocks a fragment may carry. Anything else is the service's, and saying
/// so by name beats a generic "unknown field".
const FRAGMENT_BLOCKS: [&str; 13] = [
    "catalog",
    "crud",
    "pii",
    "emits",
    "consumes",
    "methods",
    "depends",
    "flags",
    "machine",
    "saga",
    "aggregate",
    "view",
    "metrics",
];

/// Merges one fragment. Every collision is an error naming both files: last one
/// wins is exactly the drift this whole project exists to catch, and split
/// across files nobody would see it.
fn merge(m: &mut Manifest, f: Fragment, from: &Path) -> Result<(), String> {
    let who = from.display();
    macro_rules! tables {
        ($($field:ident),*) => {$(
            for (k, v) in f.$field {
                if m.$field.contains_key(&k) {
                    return Err(format!(
                        "{who}: `{}` is already declared in {} or in another fragment. \
                         Two declarations of the same thing, and whichever won would \
                         depend on the order the files got read in",
                        k,
                        m.origin.display()
                    ));
                }
                m.$field.insert(k, v);
            }
        )*};
    }
    tables!(emits, consumes, methods, flags, machine, saga, aggregate, view, metrics, catalog);
    for d in f.depends {
        if m.depends
            .iter()
            .any(|o| o.target() == d.target() && o.method == d.method)
        {
            return Err(format!(
                "{who}: it already depends on `{}.{}`. Declared twice, the generated client \
                 would carry the same method with two policies",
                d.target(),
                d.method
            ));
        }
        m.depends.push(d);
    }
    for p in f.pii {
        if !m.pii.contains(&p) {
            m.pii.push(p);
        }
    }
    Ok(())
}

/// Resolves `include` into the files to read: an entry is a file or a
/// directory, and a directory is every `*.toml` inside it, sorted.
fn included(m: &Manifest) -> Result<Vec<PathBuf>, String> {
    let base = m.origin.parent().unwrap_or(Path::new("."));
    let mut out = Vec::new();
    for entry in &m.include {
        let path = base.join(entry);
        if path.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .collect();
            files.sort();
            if files.is_empty() {
                return Err(format!(
                    "{}: `include = \"{entry}\"` is a directory with no *.toml; an include \
                     that brings nothing hides the file somebody thought they had split out",
                    m.origin.display()
                ));
            }
            out.extend(files);
        } else if path.is_file() {
            out.push(path);
        } else {
            return Err(format!(
                "{}: `include = \"{entry}\"` does not exist",
                m.origin.display()
            ));
        }
    }
    Ok(out)
}

pub fn load(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut m: Manifest = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    // serde cannot tell "absent" from "equal to the default"; the text can
    m.cap.declared = text.contains("[cap]");
    m.origin = path.to_path_buf();
    for file in included(&m)? {
        let piece =
            std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        // The keys get checked before deserializing so the error can say WHY:
        // `[infra]` in a fragment is not an unknown field, it is a block that
        // belongs to the service.
        let table: toml::Table =
            toml::from_str(&piece).map_err(|e| format!("{}: {e}", file.display()))?;
        for key in table.keys() {
            if !FRAGMENT_BLOCKS.contains(&key.as_str()) {
                return Err(format!(
                    "{}: `{key}` belongs to the service, not to one of its features, so it \
                     goes in {}. A fragment carries: {}",
                    file.display(),
                    path.display(),
                    FRAGMENT_BLOCKS.join(", ")
                ));
            }
        }
        let f: Fragment = toml::from_str(&piece).map_err(|e| format!("{}: {e}", file.display()))?;
        merge(&mut m, f, &file)?;
    }
    // Last, and before anybody reads the manifest: what a CRUD stands for is
    // five ordinary methods, and every rule that already applies to a method
    // has to apply to them. Expanding here is what buys that for free.
    expand(&mut m)?;
    Ok(m)
}

/// Merges manifests from disk and from live services. A service publishes its
/// own at /.well-known/axon.json; an external one is frozen into a *.external.toml.
/// One manifest, from a path or from a URL.
///
/// The subject of a `build` can be a running service too: what it serves at
/// `/.well-known/axon.json` is its contract right now, which is the point of
/// serving it. Whoever generates a client against a live peer wants the peer's
/// answer and not the copy somebody remembered to commit.
pub fn load_any(source: &str) -> Result<Manifest, String> {
    if source.starts_with("http://") || source.starts_with("https://") {
        discover(std::slice::from_ref(&source.to_string()))?
            .into_iter()
            .next()
            .ok_or_else(|| format!("{source}: it served no manifest"))
    } else {
        load(Path::new(source))
    }
}

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

/// The inverse of `civil`: days since the epoch from a civil date. Hinnant
/// again, because the two headers of a retirement do not take an ISO date —
/// `Deprecation` is an sf-date in seconds and `Sunset` is an HTTP-date.
pub fn epoch_days((y, m, d): (i64, i64, i64)) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `Sun, 31 Dec 2027 00:00:00 GMT`: the format RFC 8594 asks for, which is the
/// one RFC 9110 calls IMF-fixdate.
pub fn http_date(ymd: (i64, i64, i64)) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = epoch_days(ymd);
    // 1970-01-01 was a Thursday, and the table starts there
    let dow = DAYS[days.rem_euclid(7) as usize];
    let (y, m, d) = ymd;
    format!(
        "{dow}, {d:02} {} {y:04} 00:00:00 GMT",
        MONTHS[(m - 1).clamp(0, 11) as usize]
    )
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
