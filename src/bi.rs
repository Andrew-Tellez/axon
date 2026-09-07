//! Export to the data warehouse, and the business metrics that come out of the
//! cadena causal declarada.
//!
//! This is derivable in full: axon already knows every event's schema, which
//! fields are personal, and —most importantly— **who causes whom**. That last
//! part is the one no warehouse has: a funnel is normally assembled by guessing
//! how the events relate, and here it is declared.
use crate::manifest::*;
use indexmap::IndexMap;

/// The envelope's columns, the same in every table. They are what makes
/// reconstructing a flow possible: without `correlation_id` there is no funnel.
const ENVELOPE: [(&str, &str); 7] = [
    ("event_id", "string"),
    ("event_type", "string"),
    ("source", "string"),
    ("event_time", "timestamp"),
    ("trace_id", "string"),
    ("correlation_id", "string"),
    ("causation_id", "string"),
];

/// What differs between warehouses. Nothing but this: the schema and the
/// funnels are the same, because they come from the same manifest.
///
/// Las diferencias no son cosmeticas. BigQuery necesita `PARTITION BY`
/// explicit partitioning or every query scans the whole history; Snowflake
/// partitions on its own and only accepts `CLUSTER BY`; ClickHouse needs an
/// engine and an ordering key, and without `Nullable` an empty column stores a
/// zero instead of nothing.
pub struct Dialect {
    pub name: &'static str,
    /// Quotes an identifier.
    pub quote: fn(&str) -> String,
    /// Column type for one of axon's types.
    pub kind: fn(&str) -> String,
    /// What goes after the column parenthesis.
    pub tail: fn() -> String,
    /// Diferencia en milisegundos entre dos expresiones.
    pub diff_ms: fn(&str, &str) -> String,
}

fn quote_backtick(s: &str) -> String {
    format!("`{s}`")
}
fn quote_double(s: &str) -> String {
    format!("\"{s}\"")
}

fn type_bigquery(t: &str) -> String {
    match t {
        "int" => "INT64",
        "float" => "FLOAT64",
        "bool" => "BOOL",
        "timestamp" => "TIMESTAMP",
        "json" => "JSON",
        _ => "STRING",
    }
    .into()
}

fn type_snowflake(t: &str) -> String {
    match t {
        "int" => "NUMBER(38,0)",
        "float" => "FLOAT",
        "bool" => "BOOLEAN",
        "timestamp" => "TIMESTAMP_TZ",
        "json" => "VARIANT",
        _ => "VARCHAR",
    }
    .into()
}

/// `Nullable(DateTime64(3))` -> `DateTime64(3)`. Stripping every trailing
/// parenthesis breaks parameterised types: exactly one has to come off.
fn without_nullable(t: &str) -> String {
    match t
        .strip_prefix("Nullable(")
        .and_then(|r| r.strip_suffix(')'))
    {
        Some(dentro) => dentro.to_string(),
        None => t.to_string(),
    }
}

fn type_clickhouse(t: &str) -> String {
    match t {
        "int" => "Nullable(Int64)",
        "float" => "Nullable(Float64)",
        "bool" => "Nullable(Bool)",
        // milliseconds: one event per second does not order a funnel properly
        "timestamp" => "Nullable(DateTime64(3))",
        "json" => "Nullable(String)",
        _ => "Nullable(String)",
    }
    .into()
}

pub fn dialect(name: &str) -> Option<Dialect> {
    Some(match name {
        "bigquery" => Dialect {
            name: "bigquery",
            quote: quote_backtick,
            kind: type_bigquery,
            tail: || {
                "-- partitioning is not optional: without it every query scans the\n\
                 -- whole table and the bill grows with the history\n\
                 PARTITION BY DATE(event_time)\n\
                 CLUSTER BY correlation_id, source"
                    .into()
            },
            diff_ms: |a, b| format!("TIMESTAMP_DIFF(\n{a},\n{b},\n    MILLISECOND\n  )"),
        },
        "snowflake" => Dialect {
            name: "snowflake",
            quote: quote_double,
            kind: type_snowflake,
            // Snowflake partitions on its own with micro-partitions: declaring
            // PARTITION BY would be an error, not an optimisation.
            tail: || "CLUSTER BY (TO_DATE(event_time), correlation_id)".into(),
            diff_ms: |a, b| format!("TIMESTAMPDIFF(\n    MILLISECOND,\n{b},\n{a}\n  )"),
        },
        "clickhouse" => Dialect {
            name: "clickhouse",
            quote: quote_double,
            kind: type_clickhouse,
            tail: || {
                // Clause order matters: ClickHouse expects ORDER BY right after
                // the engine. And the ordering key decides which queries are
                // fast: the flow first, because a funnel groups by it.
                "ENGINE = MergeTree\n\
                 ORDER BY (correlation_id, event_time)\n\
                 PARTITION BY toYYYYMM(event_time)"
                    .into()
            },
            diff_ms: |a, b| format!("dateDiff(\n    'millisecond',\n{b},\n{a}\n  )"),
        },
        _ => return None,
    })
}


/// The local target's loader: it carries the envelope log into ClickHouse.
///
/// The log is written by the target itself —`AXON_TRACE_LOG` is in the
/// generated compose, it is not a demo artefact— so the local warehouse is
/// filled from the same source as the trace. And the columns and their paths
/// inside the JSON come from the same place as the schema: if the schema
/// changes, this changes with it, which is the only way they do not drift.
///
/// It exists because generating the schema with no path to fill it leaves
/// empty tables without a single error, and that is indistinguishable from
/// "nothing happened".
pub fn loader(ms: &[Manifest], base: &str, log: &str) -> String {
    let d = dialect("clickhouse").expect("clickhouse");
    let mut o = vec![
        "-- generated by axon — do not edit.".to_string(),
        format!("--   axon analytics manifests/ --load {log} > load.sql"),
        "--".to_string(),
        "-- Idempotent per event: it filters by what has already been loaded, so".to_string(),
        "-- running it twice does not duplicate rows. Without that, a periodic".to_string(),
        "-- loader multiplies each event by the number of passes and the funnel lies.".to_string(),
        String::new(),
    ];
    for e in eventos(ms) {
        let t = table(e.name);
        let mut sel = vec![
            "  JSONExtractString(l, 'id')            AS event_id".to_string(),
            "  JSONExtractString(l, 'type')          AS event_type".to_string(),
            "  JSONExtractString(l, 'source')        AS source".to_string(),
            "  parseDateTime64BestEffort(JSONExtractString(l, 'time'), 3) AS event_time".to_string(),
            // the trace_id is the second field of the W3C traceparent
            "  splitByChar('-', JSONExtractString(l, 'traceparent'))[2] AS trace_id".to_string(),
            "  JSONExtractString(l, 'correlationId') AS correlation_id".to_string(),
            "  nullIf(JSONExtractString(l, 'causationId'), '') AS causation_id".to_string(),
        ];
        for (field, kind) in e.fields {
            let sensible = is_pii(&e.pii, field);
            if sensible && e.modo_pii == "exclude" {
                continue;
            }
            for (n, _) in columns(&d, field, kind) {
                // the path inside the JSON: `data.<field>`, and `money` is
                // flattened into the two the schema declares
                let route = if n.ends_with("_currency") {
                    format!("'data', '{field}', 'currency'")
                } else if kind == "money" {
                    format!("'data', '{field}', 'amount'")
                } else {
                    format!("'data', '{field}'")
                };
                if sensible {
                    sel.push(format!(
                        "  lower(hex(SHA256(concat({{salt:String}}, JSONExtractString(l, {route}))))) AS {n}_hash"
                    ));
                } else if kind == "int" || (kind == "money" && !n.ends_with("_currency")) {
                    sel.push(format!("  JSONExtractInt(l, {route}) AS {n}"));
                } else {
                    sel.push(format!("  nullIf(JSONExtractString(l, {route}), '') AS {n}"));
                }
            }
        }
        o.push(format!(
            "-- {} · owner: {}\nINSERT INTO {base}.{t}\nSELECT\n{}\nFROM file('{log}', LineAsString, 'l String')\nWHERE JSONExtractString(l, 'type') = '{}'\n  -- what is already loaded is not loaded again\n  AND JSONExtractString(l, 'id') NOT IN (SELECT event_id FROM {base}.{t});\n",
            e.name,
            e.duenio,
            sel.join(",\n"),
            e.name
        ));
    }
    o.join("\n")
}


/// The query that dumps the warehouse's REAL schema.
///
/// It is emitted rather than run for the same reason as `axon load --check`:
/// axon does not have —nor want— warehouse credentials. The output comes back
/// through `--check`, and the compiler does the diff.
pub fn introspect(d: &Dialect, base: &str) -> String {
    let cuerpo = match d.name {
        "clickhouse" => format!(
            "SELECT table, name, type FROM system.columns\n \
             WHERE database = '{base}'\n \
             ORDER BY table, position\n\
             FORMAT TSV"
        ),
        "bigquery" => format!(
            "SELECT table_name, column_name, data_type\n  \
               FROM `{base}`.INFORMATION_SCHEMA.COLUMNS\n \
              ORDER BY table_name, ordinal_position"
        ),
        _ => format!(
            "SELECT table_name, column_name, data_type\n  \
               FROM information_schema.columns\n \
              WHERE table_schema = '{base}'\n \
              ORDER BY table_name, ordinal_position"
        ),
    };
    format!(
        "-- generated by axon — do not edit.\n\
         --   axon analytics manifests/ --introspect > esquema.sql\n\
         --   ... run it against the warehouse and save the output ...\n\
         --   axon analytics manifests/ --check esquema.tsv\n\
         --\n\
         -- Tres columns, en este orden: table, columna, kind.\n\
         {cuerpo};\n"
    )
}

/// A type's family, which is the only comparable thing across warehouses.
///
/// `Nullable(String)`, `STRING` and `text` are the same type with three names.
/// What CANNOT be confused is a date with a text or an integer with a string,
/// and that is exactly what breaks a query without raising an error: a
/// `event_time` stored as text sorts wrong.
fn family(t: &str) -> &'static str {
    let t = t.to_lowercase();
    let t = t
        .trim_start_matches("nullable(")
        .trim_end_matches(')')
        .trim();
    if t.contains("date") || t.contains("time") {
        "a date"
    } else if t.contains("int") || t.contains("numeric") || t.contains("decimal") || t.contains("float") {
        "a number"
    } else if t.contains("bool") {
        "a boolean"
    } else {
        "text"
    }
}

/// The declared against what is actually in the warehouse.
///
/// Drift here raises an error nowhere: a new column the table does not have
/// loads as nothing, and an old column nobody writes any more keeps the data it
/// had. Both give queries that return numbers, which is why nobody looks at
/// them.
pub fn review(ms: &[Manifest], d: &Dialect, real: &str) -> (Vec<String>, Vec<String>) {
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());
    // what is there: table -> column -> type
    let mut present: IndexMap<String, IndexMap<String, String>> = IndexMap::new();
    for line in real.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('-') || l.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = l.split(['\t', ',']).map(str::trim).collect();
        if fields.len() < 3 {
            continue;
        }
        present.entry(fields[0].to_lowercase())
            .or_default()
            .insert(fields[1].to_lowercase(), fields[2].to_string());
    }
    if present.is_empty() {
        errors.push(
            "the dump has no columns at all. Run `axon analytics --introspect` against the \
             warehouse and pass its output: comparing against an empty file gives 0 \
             differences and that reads as everything being fine"
                .into(),
        );
        return (errors, warnings);
    }

    for e in eventos(ms) {
        let t = table(e.name);
        let Some(real_cols) = present.get(&t) else {
            errors.push(format!(
                "{}: the `{t}` table is missing from the warehouse. The event is emitted and \
                 ninguna parte",
                e.name
            ));
            continue;
        };
        let mut declared: IndexMap<String, String> = IndexMap::new();
        for (n, t) in ENVELOPE {
            declared.insert(n.to_string(), (d.kind)(t));
        }
        for (field, kind) in e.fields {
            let sensible = is_pii(&e.pii, field);
            if sensible && e.modo_pii == "exclude" {
                // Declared as excluded and present in the warehouse: the
                // personal data is there from an earlier version, and stays.
                for (n, _) in columns(d, field, kind) {
                    if real_cols.contains_key(&n) {
                        errors.push(format!(
                            "{}: `{t}.{n}` exists in the warehouse and the manifest declares that field \
                             as excluded. The personal data was left there by an earlier version \
                             and does not leave on its own: the column has to be dropped",
                            e.name
                        ));
                    }
                }
                continue;
            }
            for (n, ty) in columns(d, field, kind) {
                let name = if sensible { format!("{n}_hash") } else { n.clone() };
                declared.insert(name, if sensible { (d.kind)("string") } else { ty });
                // the plaintext value cannot stay there after moving to a hash
                if sensible && real_cols.contains_key(&n) {
                    errors.push(format!(
                        "{}: `{t}.{n}` exists in plaintext and the manifest declares `pii = \"hash\"`. \
                         The new column gets filled and the old one keeps the addresses it \
                         already had",
                        e.name
                    ));
                }
            }
        }
        for (n, ty) in &declared {
            match real_cols.get(n) {
                None => errors.push(format!(
                    "{}: `{t}.{n}` is missing from the warehouse. What is declared gets loaded \
                     there, and without the column that field is stored nowhere",
                    e.name
                )),
                Some(real_ty) if family(real_ty) != family(ty) => errors.push(format!(
                    "{}: `{t}.{n}` is {} in the warehouse and the manifest declares {}. A \
                     date stored as text sorts wrong and raises no error",
                    e.name,
                    family(real_ty),
                    family(ty)
                )),
                _ => {}
            }
        }
        for n in real_cols.keys() {
            if !declared.contains_key(n) {
                warnings.push(format!(
                    "{}: `{t}.{n}` is in the warehouse and not in the manifest. Left over from an \
                     earlier version: it breaks nothing and keeps being queried",
                    e.name
                ));
            }
        }
    }
    (errors, warnings)
}


/// Vector config: the ingest path for a cluster.
///
/// A cluster brings no managed warehouse, so there is nothing to subscribe with
/// —that is why `k8s` had no path at all. axon does not ship a consumer either:
/// it generates config for a real tool, the same way it does for flagd, pgdog
/// and Flyway. Vector is a single binary driven by config, and `vector
/// validate` checks it — so this generator is verified by the parser that will
/// read it, not by our idea of its shape.
///
/// One source per event, not one with a wildcard and a router. A router leaves
/// an `_unmatched` branch, and an event that arrives there is dropped in
/// silence: `vector validate` warns about it, and a warning today is a lost
/// event tomorrow.
pub fn vector(ms: &[Manifest], base: &str) -> String {
    let d = dialect("clickhouse").expect("clickhouse");
    let evs = eventos(ms);
    let mut sources = Vec::new();
    let mut transforms = Vec::new();
    let mut sinks = Vec::new();
    for e in &evs {
        let t = table(e.name);
        // NATS rejects `@` in a subject, the same substitution the runtime does
        let subject = e.name.replace('@', ".");
        sources.push(format!(
            "  in_{t}:\n    \
               type: nats\n    \
               url: ${{AXON_BROKER_URL:-nats://broker:4222}}\n    \
               subject: {subject}\n    \
               # A queue group: with several replicas, each event is delivered\n    \
               # once. Without it every replica writes the same row and the\n    \
               # funnel counts every flow as many times as there are replicas.\n    \
               queue: axon-warehouse\n    \
               connection_name: axon-warehouse\n"
        ));

        let mut fields = vec![
            "        \"event_id\": e.id,".to_string(),
            "        \"event_type\": e.type,".to_string(),
            "        \"source\": e.source,".to_string(),
            "        \"event_time\": e.time,".to_string(),
            // the trace id is the second field of the W3C traceparent
            "        \"trace_id\": trace[1],".to_string(),
            "        \"correlation_id\": e.correlationId,".to_string(),
            "        \"causation_id\": e.causationId,".to_string(),
        ];
        for (field, kind) in e.fields {
            let sensible = is_pii(&e.pii, field);
            if sensible && e.modo_pii == "exclude" {
                continue;
            }
            for (n, _) in columns(&d, field, kind) {
                let route = if n.ends_with("_currency") {
                    format!("e.data.{field}.currency")
                } else if kind == "money" {
                    format!("e.data.{field}.amount")
                } else {
                    format!("e.data.{field}")
                };
                if sensible {
                    fields.push(format!(
                        "        \"{n}_hash\": sha2(join!([get_env_var!(\"AXON_PII_SALT\"), \
                         string!({route})]), variant: \"SHA-256\"),"
                    ));
                } else {
                    fields.push(format!("        \"{n}\": {route},"));
                }
            }
        }
        transforms.push(format!(
            "  map_{t}:\n    \
               type: remap\n    \
               inputs: [in_{t}]\n    \
               source: |\n      \
                 e = parse_json!(string!(.message))\n      \
                 trace = split(string!(e.traceparent), \"-\")\n      \
                 . = {{\n{}\n      }}\n",
            fields.join("\n")
        ));

        sinks.push(format!(
            "  out_{t}:\n    \
               type: clickhouse\n    \
               inputs: [map_{t}]\n    \
               endpoint: ${{AXON_WAREHOUSE_URL:-http://bodega:8123}}\n    \
               database: {base}\n    \
               table: {t}\n    \
               # A field the table does not have is an error, not something to\n    \
               # drop: it means the schema and the manifest drifted apart, and\n    \
               # `axon analytics --check` is what says which way.\n    \
               skip_unknown_fields: false\n    \
               date_time_best_effort: true\n    \
               auth: {{ strategy: basic, user: ${{AXON_WAREHOUSE_USER}}, password: ${{AXON_WAREHOUSE_PASSWORD}} }}\n    \
               # On disk, and blocking when full. In memory, a restart loses\n    \
               # whatever had not been written; dropping on a full buffer loses\n    \
               # events under exactly the load that makes them worth counting.\n    \
               buffer: {{ type: disk, max_size: 268435488, when_full: block }}\n"
        ));
    }
    format!(
        "# generated by axon — do not edit.\n\
         #   axon analytics manifests/ --vector > vector.yaml\n\
         #\n\
         # Checked with `vector validate`: the parser that reads this file is the\n\
         # one that says whether it is right.\n\
         \n\
         sources:\n{}\n\
         transforms:\n{}\n\
         sinks:\n{}",
        sources.join(""),
        transforms.join(""),
        sinks.join("")
    )
}

/// Table name from the event: `order.placed@v1` -> `order_placed_v1`.
fn table(ev: &str) -> String {
    tfname(ev)
}

/// `customerId` -> `customer_id`. A warehouse is queried by hand and with BI
/// tools: there the convention is snake_case, same as in the database. The
/// contracts use the language's; the warehouse, its own.
fn snake(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                o.push('_');
            }
            o.extend(c.to_lowercase());
        } else {
            o.push(c);
        }
    }
    o
}

/// One event field to columns. `money` is flattened into two, which is what
/// makes an amount usable in a warehouse: you cannot sum an object.
fn columns(d: &Dialect, name: &str, kind: &str) -> Vec<(String, String)> {
    let n = snake(name);
    if kind == "money" {
        // `amount` already says it is an amount: `amount_amount` adds nothing
        let importe = if n.ends_with("amount") {
            n.clone()
        } else {
            format!("{n}_amount")
        };
        vec![
            (importe, (d.kind)("int")),
            (format!("{n}_currency"), (d.kind)("string")),
        ]
    } else {
        vec![(n, (d.kind)(kind))]
    }
}

struct Event<'a> {
    name: &'a str,
    duenio: &'a str,
    fields: &'a Fields,
    pii: Vec<String>,
    modo_pii: &'a str,
}

fn eventos<'a>(ms: &'a [Manifest]) -> Vec<Event<'a>> {
    let mut v = Vec::new();
    for m in ms.iter().filter(|m| !m.external && m.analytics.export) {
        let pii = m.pii.clone();
        for (ev, fields) in &m.emits {
            v.push(Event {
                name: ev,
                duenio: &m.service,
                fields,
                pii: pii.clone(),
                modo_pii: &m.analytics.pii,
            });
        }
    }
    v
}

/// BigQuery DDL: one table per event, plus the funnel views.
pub fn build(ms: &[Manifest], d: &Dialect) -> String {
    let evs = eventos(ms);
    let mut o = vec![
        "-- generated by axon — do not edit.".to_string(),
        format!(
            "--   axon analytics manifests/ --target {} > bodega.sql",
            d.name
        ),
        "--".to_string(),
        "-- One table per event, with the envelope columns that make reconstructing".to_string(),
        "-- a flow possible, and the funnel views that come out of the DECLARED".to_string(),
        "-- causal chain. A funnel is normally assembled by guessing how the".to_string(),
        "-- events relate; here it is written in the manifest.".to_string(),
        String::new(),
        "-- The dataset is passed as a parameter: `bq query --parameter=dataset::my_dataset`"
            .to_string(),
        "-- or substituted before applying.".to_string(),
    ];
    if evs.is_empty() {
        o.push("\n-- Ningun servicio exporta eventos.".into());
        o.push(String::new());
        return o.join("\n");
    }

    for e in &evs {
        let mut cols: Vec<String> = ENVELOPE
            .iter()
            .map(|(n, t)| {
                let nullable = matches!(*n, "trace_id" | "causation_id");
                let kind = (d.kind)(t);
                // in ClickHouse nullability goes in the type, not in a suffix
                if d.name == "clickhouse" {
                    let kind = if nullable { kind } else { without_nullable(&kind) };
                    format!("  {n} {kind}")
                } else {
                    format!("  {n} {kind}{}", if nullable { "" } else { " NOT NULL" })
                }
            })
            .collect();
        let mut excluidos = Vec::new();
        for (field, kind) in e.fields {
            let sensible = is_pii(&e.pii, field);
            if sensible && e.modo_pii == "exclude" {
                excluidos.push(field.clone());
                continue;
            }
            for (n, t) in columns(d, field, kind) {
                if sensible {
                    // a salted hash, not the value: a warehouse is where
                    // personal data lives longest and is read by the most people
                    cols.push(format!(
                        "  -- salted SHA-256 of `{field}`: countable without storing it\n  {n}_hash {}",
                        (d.kind)("string")
                    ));
                } else {
                    cols.push(format!("  {n} {t}"));
                }
            }
        }
        o.push(format!("\n-- {} · owner: {}", e.name, e.duenio));
        if !excluidos.is_empty() {
            o.push(format!(
                "-- Personal fields excluded: {}. With `[analytics] pii = \"hash\"` they would\n\
                 -- be exported as a salted SHA-256 instead of not at all.",
                excluidos.join(", ")
            ));
        }
        o.push(format!(
            "CREATE TABLE IF NOT EXISTS {} (\n{}\n)\n{};",
            (d.quote)(&format!("@dataset.{}", table(e.name))),
            cols.join(",\n"),
            (d.tail)()
        ));
    }

    o.extend(embudos(ms, &evs, d));
    o.push(String::new());
    o.join("\n")
}

/// Funnel views: one row per business flow, with the moment of each step and
/// the time between them.
///
/// The steps come from the declared causal chain, the same one `axon seq`
/// draws. That is what makes the funnel not a guess.
fn embudos(ms: &[Manifest], evs: &[Event], d: &Dialect) -> Vec<String> {
    let emisor: IndexMap<&str, &str> = evs.iter().map(|e| (e.name, e.duenio)).collect();
    let mut o = Vec::new();

    // an event is a root if nobody consumes it to produce it, that is, if no
    // service emits it as a consequence of another
    let derivados: Vec<&str> = ms
        .iter()
        .flat_map(|m| {
            m.emits
                .keys()
                .filter(|_| !m.consumes.is_empty())
                .map(|e| e.as_str())
        })
        .collect();
    let raices: Vec<&str> = emisor
        .keys()
        .copied()
        .filter(|e| !derivados.contains(e))
        .collect();

    for root in raices {
        let mut cadena = vec![root];
        let mut i = 0;
        while i < cadena.len() && cadena.len() < 12 {
            let actual = cadena[i];
            for m in ms {
                if m.consumes.contains_key(actual) {
                    for sig in m.emits.keys() {
                        if !cadena.contains(&sig.as_str()) && emisor.contains_key(sig.as_str()) {
                            cadena.push(sig);
                        }
                    }
                }
            }
            i += 1;
        }
        if cadena.len() < 2 {
            continue;
        }

        let union: Vec<String> = cadena
            .iter()
            .map(|e| {
                format!(
                    "    SELECT correlation_id, event_type, event_time FROM {}",
                    (d.quote)(&format!("@dataset.{}", table(e)))
                )
            })
            .collect();
        let pasos: Vec<String> = cadena
            .iter()
            .enumerate()
            .map(|(n, e)| {
                // CASE WHEN and not IF()/IFF(): it is the only thing all three
                // entienden igual
                format!(
                    "  MIN(CASE WHEN event_type = '{e}' THEN event_time END) AS paso_{}_{}",
                    n + 1,
                    table(e)
                )
            })
            .collect();
        // the time between the first step and each of the following ones: that
        // is the business flow's latency, not a request's
        let saltos: Vec<String> = cadena
            .iter()
            .skip(1)
            .map(|e| {
                let hasta = format!("    MIN(CASE WHEN event_type = '{e}' THEN event_time END)");
                let desde = format!(
                    "    MIN(CASE WHEN event_type = '{}' THEN event_time END)",
                    cadena[0]
                );
                format!("  {} AS ms_hasta_{}", (d.diff_ms)(&hasta, &desde), table(e))
            })
            .collect();

        o.push(format!(
            "\n-- Funnel for `{root}`: one flow per row.\n\
             -- The steps come from the causal chain declared in the manifests, the\n\
             -- same one `axon seq` draws. A NULL step is a flow that did not get\n\
             -- there: that is the conversion, and the TIMESTAMP_DIFF is the business\n\
             -- latency, not a request\'s.\n\
             CREATE OR REPLACE VIEW {} AS\n\
             SELECT\n  correlation_id,\n{},\n{}\nFROM (\n{}\n)\nGROUP BY correlation_id;",
            (d.quote)(&format!("@dataset.embudo_{}", table(root))),
            pasos.join(",\n"),
            saltos.join(",\n"),
            union.join("\n    UNION ALL\n")
        ));
    }
    o
}

/// The export's neutral plan, for whoever does not use BigQuery.
pub fn build_plan(ms: &[Manifest]) -> serde_json::Value {
    let evs = eventos(ms);
    // the plan carries axon's types, not a warehouse's: whoever consumes it
    // translates them into their own
    let neutral = Dialect {
        name: "plan",
        quote: |s| s.to_string(),
        kind: |t| t.to_string(),
        tail: String::new,
        diff_ms: |a, b| format!("{a} - {b}"),
    };
    serde_json::json!({
        "envelope": ENVELOPE.iter().map(|(n, t)| serde_json::json!({"name": n, "type": t})).collect::<Vec<_>>(),
        "tables": evs.iter().map(|e| serde_json::json!({
            "event": e.name,
            "owner": e.duenio,
            "table": table(e.name),
            "partition_by": "DATE(event_time)",
            "cluster_by": ["correlation_id", "source"],
            "pii_mode": e.modo_pii,
            "columns": e.fields.iter().flat_map(|(n, t)| {
                let sensible = is_pii(&e.pii, n);
                columns(&neutral, n, t).into_iter().filter_map(move |(cn, ct)| {
                    match (sensible, e.modo_pii) {
                        (true, "exclude") => None,
                        (true, _) => Some(serde_json::json!({"name": format!("{cn}_hash"), "type": "STRING", "pii": true})),
                        _ => Some(serde_json::json!({"name": cn, "type": ct})),
                    }
                }).collect::<Vec<_>>()
            }).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}
