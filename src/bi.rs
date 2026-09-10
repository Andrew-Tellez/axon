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
    /// What goes after the column parenthesis, given how long the table keeps
    /// its rows. It is per dialect because the three say it in three places and
    /// one of them does not say it at all: ClickHouse has a `TTL` in the table,
    /// BigQuery an option on the partition, and Snowflake's
    /// `DATA_RETENTION_TIME_IN_DAYS` is Time Travel —how far back you can
    /// query, capped at 90 days— which is a different thing entirely. Using it
    /// as retention would delete nothing and read as if it did.
    pub tail: fn(Option<i64>) -> String,
    /// Diferencia en milisegundos entre dos expresiones.
    pub diff_ms: fn(&str, &str) -> String,
    /// The time bucket of a metric: `("1d", "event_time")`. It is per dialect
    /// because the three truncate a timestamp with three different functions,
    /// and a bucket that means "day" in one warehouse and "day in UTC minus the
    /// session's timezone" in another is a metric that does not match itself.
    pub bucket: fn(&str, &str) -> String,
    /// The value `n` windows back, ordered by bucket. Per dialect because
    /// ClickHouse has no `LAG`.
    pub lag: fn(&str, u32) -> String,
}

fn quote_backtick(s: &str) -> String {
    format!("`{s}`")
}
fn quote_double(s: &str) -> String {
    format!("\"{s}\"")
}

/// `dataset.table`, with each half quoted SEPARATELY.
///
/// Quoting the whole thing is one identifier that happens to contain a dot,
/// not a table inside a dataset. In BigQuery the backtick form means both, so
/// it went unnoticed; in Snowflake and ClickHouse it does not. Measured
/// against a ClickHouse 24: `CREATE TABLE "bench.demo"` lands in `default`
/// under the literal name `bench.demo`, and `SELECT FROM bench.demo` then
/// answers `UNKNOWN_TABLE`. The schema applies with no error and the dataset
/// it was aimed at stays empty, which is the one failure this module exists
/// to avoid.
fn qualify(d: &Dialect, name: &str) -> String {
    name.split('.')
        .map(d.quote)
        .collect::<Vec<_>>()
        .join(".")
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
            tail: |keeps| {
                // The expiry is per PARTITION, which is why partitioning is not
                // optional here for two reasons and not one.
                let mut t = String::from(
                    "-- partitioning is not optional: without it every query scans the\n\
                     -- whole table and the bill grows with the history\n\
                     PARTITION BY DATE(event_time)\n\
                     CLUSTER BY correlation_id, source",
                );
                if let Some(d) = keeps {
                    t.push_str(&format!(
                        "\nOPTIONS (\n  partition_expiration_days = {d},\n  \
                         description = \"kept {d} days, declared in the manifest\"\n)"
                    ));
                }
                t
            },
            diff_ms: |a, b| format!("TIMESTAMP_DIFF(\n{a},\n{b},\n    MILLISECOND\n  )"),
            // TIMESTAMP_TRUNC does not take WEEK or MONTH, and DATE_TRUNC over a
            // DATE does: the bucket comes back as a DATE for those two, which is
            // what a weekly or monthly metric wants anyway.
            bucket: |w, c| match w {
                "1h" => format!("TIMESTAMP_TRUNC({c}, HOUR)"),
                "1w" => format!("DATE_TRUNC(DATE({c}), WEEK)"),
                "1mo" => format!("DATE_TRUNC(DATE({c}), MONTH)"),
                _ => format!("TIMESTAMP_TRUNC({c}, DAY)"),
            },
            lag: |c, n| format!("LAG({c}, {n}) OVER (ORDER BY bucket)"),
        },
        "snowflake" => Dialect {
            name: "snowflake",
            quote: quote_double,
            kind: type_snowflake,
            // Snowflake partitions on its own with micro-partitions: declaring
            // PARTITION BY would be an error, not an optimisation.
            // Snowflake has no expiry in the table: `DATA_RETENTION_TIME_IN_DAYS`
            // is Time Travel —how far back you can query— and caps at 90 days.
            // Using it as retention would delete nothing and read as if it did,
            // so what comes out is a comment here and a real TASK below.
            // The comment goes BEFORE and not after: what closes the statement
            // is the `;` that comes right behind this, and a trailing comment
            // swallows it. The DDL then reads as one statement that never ends,
            // which is the same trap this file already documents for a comment
            // at the end of a column.
            tail: |keeps| match keeps {
                Some(d) => format!(
                    "-- kept {d} days by the task below. NOT with\n\
                     -- DATA_RETENTION_TIME_IN_DAYS: that is Time Travel, it caps at 90\n\
                     -- days and it deletes nothing\n\
                     CLUSTER BY (TO_DATE(event_time), correlation_id)"
                ),
                None => "CLUSTER BY (TO_DATE(event_time), correlation_id)".into(),
            },
            diff_ms: |a, b| format!("TIMESTAMPDIFF(\n    MILLISECOND,\n{b},\n{a}\n  )"),
            bucket: |w, c| {
                let unit = match w {
                    "1h" => "HOUR",
                    "1w" => "WEEK",
                    "1mo" => "MONTH",
                    _ => "DAY",
                };
                format!("DATE_TRUNC('{unit}', {c})")
            },
            lag: |c, n| format!("LAG({c}, {n}) OVER (ORDER BY bucket)"),
        },
        "clickhouse" => Dialect {
            name: "clickhouse",
            quote: quote_double,
            kind: type_clickhouse,
            tail: |keeps| {
                // Clause order matters: ClickHouse expects ORDER BY right after
                // the engine, and the TTL after the partition. And the ordering
                // key decides which queries are fast: the flow first, because a
                // funnel groups by it.
                let mut t = String::from(
                    "ENGINE = MergeTree\n\
                     ORDER BY (correlation_id, event_time)\n\
                     PARTITION BY toYYYYMM(event_time)",
                );
                if let Some(d) = keeps {
                    // `toDateTime` and not the column as it is: the column is a
                    // DateTime64(3) —milliseconds, because two events in the same
                    // second have an order— and ClickHouse's TTL only takes Date
                    // or DateTime. It refuses with BAD_TTL_EXPRESSION.
                    t.push_str(&format!("\nTTL toDateTime(event_time) + INTERVAL {d} DAY"));
                }
                t
            },
            diff_ms: |a, b| format!("dateDiff(\n    'millisecond',\n{b},\n{a}\n  )"),
            bucket: |w, c| {
                let f = match w {
                    "1h" => "toStartOfHour",
                    "1w" => "toStartOfWeek",
                    "1mo" => "toStartOfMonth",
                    _ => "toStartOfDay",
                };
                format!("{f}({c})")
            },
            // The frame written out: with the default one `lagInFrame` answers
            // the current row, and a rule comparing a number against itself
            // never fires and reads exactly like everything being fine.
            lag: |c, n| {
                format!(
                    "lagInFrame({c}) OVER (ORDER BY bucket ROWS BETWEEN {n} PRECEDING AND {n} PRECEDING)"
                )
            },
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
            "  parseDateTime64BestEffort(JSONExtractString(l, 'time'), 3) AS event_time"
                .to_string(),
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
                    sel.push(format!(
                        "  nullIf(JSONExtractString(l, {route}), '') AS {n}"
                    ));
                }
            }
        }
        // The columns go NAMED. `INSERT ... SELECT` matches by POSITION, so a
        // table whose columns were reordered —which is what a `DROP COLUMN`
        // followed by an `ADD COLUMN` leaves, because the column comes back at
        // the end— loads the amount into the currency and nobody sees an error.
        // Measured: after the drift check dropped and re-added `total_amount`,
        // the currency column held `100` and the metric over the amount answered
        // NULL. Named, a column that moved is harmless and one that is missing is
        // an error.
        let names: Vec<String> = sel
            .iter()
            .map(|c| c.rsplit(" AS ").next().unwrap_or_default().to_string())
            .collect();
        o.push(format!(
            "-- {} · owner: {}\nINSERT INTO {base}.{t} ({})\nSELECT\n{}\nFROM file('{log}', LineAsString, 'l String')\nWHERE JSONExtractString(l, 'type') = '{}'\n  -- what is already loaded is not loaded again\n  AND JSONExtractString(l, 'id') NOT IN (SELECT event_id FROM {base}.{t});\n",
            e.name,
            e.duenio,
            names.join(", "),
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
    } else if t.contains("int")
        || t.contains("numeric")
        || t.contains("decimal")
        || t.contains("float")
    {
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
        present
            .entry(fields[0].to_lowercase())
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
                let name = if sensible {
                    format!("{n}_hash")
                } else {
                    n.clone()
                };
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
               endpoint: ${{AXON_WAREHOUSE_URL:-http://warehouse:8123}}\n    \
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
pub fn snake(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                o.push('_');
            }
            o.extend(c.to_lowercase());
        } else if c == '.' {
            // `total.currency` is how a `money` sub-field is named in the
            // manifest, because that is the shape the contract declares; in the
            // warehouse it is one flat column.
            o.push('_');
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
    /// How many days it is kept, if anybody said. `None` is forever, and
    /// forever is a decision nobody took.
    keeps: Option<i64>,
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
                keeps: m.analytics.keeps(ev),
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
            "--   axon analytics manifests/ --target {} > warehouse.sql",
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
        o.push("\n-- No service exports events.".into());
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
                    let kind = if nullable {
                        kind
                    } else {
                        without_nullable(&kind)
                    };
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
            qualify(d, &format!("@dataset.{}", table(e.name))),
            cols.join(",\n"),
            (d.tail)(e.keeps)
        ));
        // `IF NOT EXISTS` ignores everything when the table is already there,
        // and retention is precisely what gets decided later: without this, the
        // day somebody declares it the schema applies with no error and the
        // table keeps growing forever. Found by applying it twice.
        if let Some(days) = e.keeps {
            let name = qualify(d, &format!("@dataset.{}", table(e.name)));
            match d.name {
                "clickhouse" => o.push(format!(
                    "ALTER TABLE {name} MODIFY TTL toDateTime(event_time) + INTERVAL {days} DAY;"
                )),
                "bigquery" => o.push(format!(
                    "ALTER TABLE {name} SET OPTIONS (partition_expiration_days = {days});"
                )),
                // Snowflake's is the task below, and it is CREATE OR REPLACE
                _ => {}
            }
        }
    }

    // Snowflake deletes nothing on its own: what the table says up there is a
    // comment, and this is where the rows actually go. One task per table
    // because a task is suspended, resumed and audited on its own, and a single
    // one deleting from six tables is six decisions with one switch.
    if d.name == "snowflake" {
        for e in evs.iter().filter(|e| e.keeps.is_some()) {
            let days = e.keeps.unwrap_or_default();
            o.push(format!(
                "\n-- {} is kept {days} days. It is a TASK and not\n\
                 -- DATA_RETENTION_TIME_IN_DAYS, which is Time Travel: it caps at 90 days\n\
                 -- and does not delete a single row.\n\
                 CREATE OR REPLACE TASK {task}\n  \
                   SCHEDULE = 'USING CRON 0 4 * * * UTC'\n  \
                   -- suspended on creation: a task that starts deleting the moment it is\n  \
                   -- applied is a decision taken by whoever ran the DDL\nAS\n  \
                   DELETE FROM {tabla}\n  WHERE event_time < DATEADD(day, -{days}, CURRENT_TIMESTAMP());",
                e.name,
                task = qualify(d, &format!("@dataset.retain_{}", table(e.name))),
                tabla = qualify(d, &format!("@dataset.{}", table(e.name))),
            ));
        }
    }

    o.extend(funnels(ms, &evs, d));
    o.extend(metrics(ms, &evs, d));
    o.push(String::new());
    o.join("\n")
}

/// The declared metrics, one view each.
///
/// A metric is not derivable the way a funnel is: the funnel comes out of who
/// causes whom, and this comes out of somebody saying which field, grouped by
/// what, in which bucket. What declaring it buys is that it lives next to the
/// funnels —same dataset, same dialect, same partitioned tables— instead of
/// being a query pasted into a dashboard, and that `axon verify` can refute it.
fn metrics(ms: &[Manifest], evs: &[Event], d: &Dialect) -> Vec<String> {
    let known: IndexMap<&str, &Event> = evs.iter().map(|e| (e.name, e)).collect();
    let mut o = Vec::new();
    for m in ms.iter().filter(|m| !m.external && m.analytics.export) {
        for (name, mt) in &m.metrics {
            // an event nobody exports has no table to read: `verify` blocks it,
            // and emitting SQL over a table that does not exist would turn that
            // error into a failure at apply time
            if mt.on.iter().any(|e| !known.contains_key(e.as_str())) {
                continue;
            }
            let bucket = (d.bucket)(&mt.window, "event_time");
            let dims: Vec<String> = mt.by.iter().map(|f| snake(f)).collect();
            // The value column carries the aggregation in its name: a dashboard
            // that reads `value` without knowing whether it is a sum or an
            // average is a dashboard that will average an average.
            let field = mt.field.clone().unwrap_or_default();
            // The type comes from the emitter's schema: `verify` already checked
            // the field exists in every event of `on` and that it is a number.
            let column = mt.on.first().and_then(|e| {
                let ev = known.get(e.as_str())?;
                let (_, kind) = ev.fields.iter().find(|(f, _)| snake(f) == snake(&field))?;
                Some(numeric_column(&field, kind))
            });
            let value = match (mt.kind.as_str(), &column) {
                ("count", _) => "count(*)".to_string(),
                (other, Some(c)) => format!("{other}({c})"),
                // with no column there is nothing to add up; `verify` blocks
                // this, and emitting `sum()` over nothing would be SQL that does
                // not parse
                (_, None) => continue,
            };
            let mut select = vec![format!("  {bucket} AS bucket")];
            select.extend(dims.iter().map(|dim| format!("  {dim}")));
            select.push(format!("  {value} AS value"));

            // One SELECT per event, unioned: the columns are the same in every
            // table because they come from the same schema, and a metric over
            // several events counts them together on purpose.
            let cols: Vec<String> = std::iter::once("event_time".to_string())
                .chain(dims.iter().cloned())
                .chain(column.clone())
                .collect();
            let froms: Vec<String> = mt
                .on
                .iter()
                .map(|e| {
                    format!(
                        "    SELECT {} FROM {}",
                        cols.join(", "),
                        qualify(d, &format!("@dataset.{}", table(e)))
                    )
                })
                .collect();
            // One event reads its table directly; several read the union of
            // them, with the same columns in every branch because they come from
            // the same schema.
            let source = match mt.on.as_slice() {
                [one] => qualify(d, &format!("@dataset.{}", table(one))),
                _ => format!("(\n{}\n)", froms.join("\n    UNION ALL\n")),
            };
            let group: Vec<String> = std::iter::once("bucket".to_string())
                .chain(dims.iter().cloned())
                .collect();
            o.push(format!(
                "\n-- Metric `{name}` ({kind}) per {window}{by}, from {on}.\n\
                 -- Declared in {svc}: the aggregation, the field and the dimensions come\n\
                 -- from the manifest, so nobody rewrites this query in a dashboard.\n\
                 CREATE OR REPLACE VIEW {view} AS\n\
                 SELECT\n{select}\n\
                 FROM {source}\n\
                 GROUP BY {group};",
                kind = mt.kind,
                window = mt.window,
                by = if dims.is_empty() {
                    String::new()
                } else {
                    format!(" by {}", dims.join(", "))
                },
                on = mt.on.join(", "),
                svc = m.service,
                view = qualify(d, &format!("@dataset.{}", Metric::view(name))),
                select = select.join(",\n"),
                group = group.join(", "),
            ));
        }
    }
    o
}

/// The column a `sum` or an `avg` adds up, resolved against the schema its
/// EMITTER declares.
///
/// A `money` field is two columns in the warehouse —you cannot sum an object— so
/// adding up `total` means adding up `total_amount`. Resolving it here, from the
/// contract, is what lets the manifest keep talking about the field the event
/// declares instead of the column the warehouse happens to have.
fn numeric_column(field: &str, kind: &str) -> String {
    let cols = columns(&dialect("clickhouse").expect("clickhouse"), field, kind);
    cols.first().map(|(n, _)| n.clone()).unwrap_or_default()
}

/// Funnel views: one row per business flow, with the moment of each step and
/// the time between them.
///
/// The steps come from the declared causal chain, the same one `axon seq`
/// draws. That is what makes the funnel not a guess.
fn funnels(ms: &[Manifest], evs: &[Event], d: &Dialect) -> Vec<String> {
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
                    qualify(d, &format!("@dataset.{}", table(e)))
                )
            })
            .collect();
        let steps: Vec<String> = cadena
            .iter()
            .enumerate()
            .map(|(n, e)| {
                // CASE WHEN and not IF()/IFF(): it is the only thing all three
                // entienden igual
                format!(
                    "  MIN(CASE WHEN event_type = '{e}' THEN event_time END) AS step_{}_{}",
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
                let to = format!("    MIN(CASE WHEN event_type = '{e}' THEN event_time END)");
                let from_ = format!(
                    "    MIN(CASE WHEN event_type = '{}' THEN event_time END)",
                    cadena[0]
                );
                format!("  {} AS ms_to_{}", (d.diff_ms)(&to, &from_), table(e))
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
            qualify(d, &format!("@dataset.funnel_{}", table(root))),
            steps.join(",\n"),
            saltos.join(",\n"),
            union.join("\n    UNION ALL\n")
        ));
    }
    o
}

/// The query that evaluates every declared rule, one row per window.
///
/// It is emitted and not run, like `introspect`: axon has no warehouse
/// credentials and does not want them. What comes back through
/// `axon rules --check` is a table of numbers, and the DECISION —how many
/// consecutive windows, and whether it was quiet before— is taken by the
/// compiler, where it can be tested without a warehouse.
///
/// The SQL answers only what SQL is good at: the value of each window and the
/// value of the reference one.
/// The SELECT of one condition: the value per window, the reference, and
/// whether it holds there. The same shape for the trigger and for a guard,
/// because a guard is the same question about another metric.
#[allow(clippy::too_many_arguments)]
fn condition_sql(
    d: &Dialect,
    rule: &str,
    series: &str,
    view: &str,
    window: &str,
    segment: &IndexMap<String, String>,
    comparison: &str,
    back: u32,
    below: Option<f64>,
    above: Option<f64>,
    value: Option<f64>,
    note: &str,
) -> Option<String> {
    let filter = if segment.is_empty() {
        String::new()
    } else {
        format!(
            "\n    WHERE {}",
            segment
                .iter()
                .map(|(k, v)| format!("{} = '{}'", snake(k), v.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(" AND ")
        )
    };
    let reference = match comparison {
        "absolute" => format!("{}", value.unwrap_or(0.0)),
        _ => (d.lag)("value", back),
    };
    let holds = match (below, above, comparison) {
        (Some(b), _, "absolute") => format!("value < {}", value.unwrap_or(0.0) * b),
        (Some(b), _, _) => format!("reference > 0 AND value < reference * {b}"),
        (_, Some(a), "absolute") => format!("value > {}", value.unwrap_or(0.0) * a),
        (_, Some(a), _) => format!("reference > 0 AND value > reference * {a}"),
        // no threshold: `verify` blocks it
        _ => return None,
    };
    Some(format!(
        "\n-- {note}\n\
         SELECT\n  \
           '{rule}' AS rule,\n  \
           '{series}' AS series,\n  \
           bucket,\n  \
           value,\n  \
           reference,\n  \
           CASE WHEN {holds} THEN 1 ELSE 0 END AS holds\n\
         FROM (\n  \
           SELECT bucket, value, {reference} AS reference\n  \
           FROM {view}{filter}\n\
         ) AS windows\n\
         -- The current window is still filling: comparing it against a whole one\n\
         -- is comparing half a day against a day, and every rule would fire every\n\
         -- morning and lift by itself at noon.\n\
         WHERE bucket < {current}\n\
         ORDER BY bucket;",
        current = (d.bucket)(window, "now()"),
    ))
}

pub fn rules_sql(ms: &[Manifest], d: &Dialect) -> String {
    let mut o = vec!["-- Generated by axon from the manifests. Do not edit.\n\
         --\n\
         -- One row per rule, SERIES and window: the trigger and each guard, with\n\
         -- the value, the reference and whether the condition holds there.\n\
         -- Whether it PROPOSES is decided by `axon rules --check` over these rows,\n\
         -- because \"two consecutive windows, quiet before, and every guard holding\n\
         -- at those same windows\" is a decision and not an aggregate."
        .to_string()];
    for m in ms.iter().filter(|m| !m.external && m.analytics.export) {
        for (name, r) in &m.rules {
            // A metric that is not declared, or one with an unpinned dimension,
            // is blocked by `verify`; emitting SQL over a view that does not
            // exist would turn that error into a failure at apply time
            let pinned = |metric: &str, segment: &IndexMap<String, String>| -> Option<String> {
                let mt = m.metrics.get(metric)?;
                if mt
                    .by
                    .iter()
                    .any(|dim| !segment.keys().any(|k| snake(k) == snake(dim)))
                {
                    return None;
                }
                Some(mt.window.clone())
            };
            let Some(window) = pinned(&r.metric, &r.segment) else {
                continue;
            };
            let view = qualify(d, &format!("@dataset.{}", Metric::view(&r.metric)));
            if let Some(sql) = condition_sql(
                d,
                name,
                "trigger",
                &view,
                &window,
                &r.segment,
                r.comparison(),
                r.back(),
                r.below,
                r.above,
                r.value,
                &format!(
                    "Rule `{name}` of {}, trigger: `{}` per {window} against {}. Segment: {}.",
                    m.service,
                    r.metric,
                    r.comparison(),
                    r.segment_label()
                ),
            ) {
                o.push(sql);
            }
            for g in &r.guards {
                let Some(gwindow) = pinned(&g.metric, &g.segment) else {
                    continue;
                };
                let gview = qualify(d, &format!("@dataset.{}", Metric::view(&g.metric)));
                if let Some(sql) = condition_sql(
                    d,
                    name,
                    &g.series(),
                    &gview,
                    &gwindow,
                    &g.segment,
                    g.comparison(),
                    g.back(),
                    g.below,
                    g.above,
                    g.value,
                    &format!(
                        "Rule `{name}`, guard: {}. It does not propose unless this holds at \
                         the same windows.",
                        g.label()
                    ),
                ) {
                    o.push(sql);
                }
            }
        }
    }
    format!("{}\n", o.join("\n"))
}

/// One window of one rule, as it came back from the warehouse.
pub struct Window {
    pub rule: String,
    /// `trigger`, or `guard:<metric>`: a rule reads several metrics, so each
    /// row has to say which one it is.
    pub series: String,
    pub bucket: String,
    pub value: f64,
    pub holds: bool,
}

/// Reads the TSV `rules_sql` produces: rule, bucket, value, reference, holds.
///
/// Tolerant on purpose about the header and about a trailing `reference` that
/// comes back NULL —there is no previous window for the first one— because
/// whoever pipes this in is a `clickhouse-client` or a `bq`, and each one
/// decorates its output differently.
pub fn parse_windows(text: &str) -> Vec<Window> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split('\t').map(|c| c.trim()).collect();
        if cols.len() < 6 {
            continue;
        }
        let Ok(value) = cols[3].parse::<f64>() else {
            continue;
        };
        out.push(Window {
            rule: cols[0].to_string(),
            series: cols[1].to_string(),
            bucket: cols[2].to_string(),
            value,
            holds: cols[5] == "1" || cols[5].eq_ignore_ascii_case("true"),
        });
    }
    out
}

/// What a rule proposes, or why it does not.
pub struct Proposal {
    pub rule: String,
    pub service: String,
    pub fires: bool,
    /// The condition no longer holds. What went up on its own has to be able to
    /// come back down on its own, and without this the lever stays where the
    /// worst day of the quarter left it.
    pub lifted: bool,
    pub why: String,
    pub action: String,
    /// The flag it moves, and to where. `None` when it proposes something else.
    pub flag: Option<(String, String, String)>,
    /// Whether the manifest allows applying it, which is only half of the lock.
    pub apply: bool,
}

/// The decision, over the windows that came back.
///
/// A rule proposes on the way IN: the condition holding for `for` consecutive
/// windows, and NOT holding in the `cooldown` windows before those. Which means
/// there is no state to keep and nothing to get out of sync — the same data
/// gives the same answer, today and in a re-run.
pub fn decide(ms: &[Manifest], windows: &[Window]) -> Vec<Proposal> {
    let mut out = Vec::new();
    for m in ms.iter().filter(|m| !m.external) {
        for (name, r) in &m.rules {
            let mine: Vec<&Window> = windows
                .iter()
                .filter(|w| w.rule == *name && w.series == "trigger")
                .collect();
            let action = match (&r.then.flag, &r.then.emits, &r.then.calls) {
                (Some(f), _, _) => format!(
                    "set `{f}` to `{}` (back to `{}` when it lifts)",
                    r.then.variant.as_deref().unwrap_or("?"),
                    r.then.restore.as_deref().unwrap_or("?")
                ),
                (_, Some(e), _) => format!("emit `{e}`"),
                (_, _, Some(c)) => format!("call `{}.{c}`", m.service),
                _ => "nothing declared".to_string(),
            };
            let need = r.sustained as usize;
            let quiet = r.cooldown.unwrap_or(1) as usize;
            if mine.len() < need + quiet {
                out.push(Proposal {
                    rule: name.clone(),
                    service: m.service.clone(),
                    fires: false,
                    // With no history there is nothing to lift either: doing
                    // anything here would be acting on the absence of data.
                    lifted: false,
                    why: format!(
                        "{} windows of history and it needs {}: not enough to tell",
                        mine.len(),
                        need + quiet
                    ),
                    action,
                    flag: None,
                    apply: false,
                });
                continue;
            }
            let tail = &mine[mine.len() - need..];
            let before = &mine[mine.len() - need - quiet..mine.len() - need];
            let holding = tail.iter().all(|w| w.holds);
            let was_quiet = before.iter().all(|w| !w.holds);
            let last = tail.last().map(|w| w.value).unwrap_or_default();
            // The guards, at the SAME windows the trigger held. A guard whose
            // data does not come back is not a guard, so it does not propose
            // either: that is the difference between watching the other
            // direction and believing you are watching it.
            let buckets: Vec<&str> = tail.iter().map(|w| w.bucket.as_str()).collect();
            let mut blocked: Option<String> = None;
            for g in &r.guards {
                let rows: Vec<&Window> = windows
                    .iter()
                    .filter(|w| w.rule == *name && w.series == g.series())
                    .filter(|w| buckets.contains(&w.bucket.as_str()))
                    .collect();
                if rows.len() < buckets.len() {
                    blocked = Some(format!(
                        "the guard {} has no data for every window, and a guard that cannot be \
                         read is not a guard",
                        g.label()
                    ));
                    break;
                }
                if !rows.iter().all(|w| w.holds) {
                    blocked = Some(format!("the guard {} does not hold", g.label()));
                    break;
                }
            }
            let why = if let (true, true, Some(b)) = (holding, was_quiet, &blocked) {
                b.clone()
            } else if !holding {
                let held = tail.iter().filter(|w| w.holds).count();
                format!("the condition holds in {held} of the last {need} windows")
            } else if !was_quiet {
                "it already held before: proposed on the way in, not once per window".to_string()
            } else {
                format!(
                    "`{}` = {last} for {segment} held the condition for {need} windows, the \
                     {quiet} before were quiet{}",
                    r.metric,
                    if r.guards.is_empty() {
                        ", and it has no guard: nothing is watching what the lever moves in \
                         the other direction"
                            .to_string()
                    } else {
                        format!(
                            ", and {} guard{} held",
                            r.guards.len(),
                            if r.guards.len() == 1 { "" } else { "s" }
                        )
                    },
                    segment = r.segment_label()
                )
            };
            out.push(Proposal {
                rule: name.clone(),
                service: m.service.clone(),
                fires: holding && was_quiet && blocked.is_none(),
                // Lifted is not "it never held": it is that the last windows do
                // not hold it any more, which is when the lever goes back.
                lifted: !tail.iter().any(|w| w.holds),
                why,
                action,
                flag: match (&r.then.flag, &r.then.variant, &r.then.restore) {
                    (Some(f), Some(v), Some(back)) => Some((f.clone(), v.clone(), back.clone())),
                    _ => None,
                },
                apply: r.mode.as_deref() == Some("apply"),
            });
        }
    }
    out
}

/// What a BI tool needs to read what the manifest declares: the connection and
/// one question per declared metric and funnel.
///
/// It is emitted and not applied, like everything else that touches a live
/// system: axon holds no credentials for the warehouse and none for the BI
/// tool either. What this buys is that the definition of a metric stops being
/// retyped in a dashboard — the question points at the view the manifest
/// generated, so two dashboards cannot disagree about what GMV means.
pub fn metabase(ms: &[Manifest], dataset: &str) -> serde_json::Value {
    let evs = eventos(ms);
    let d = dialect("clickhouse").expect("clickhouse is a native dialect");
    let mut cards = Vec::new();
    for m in ms.iter().filter(|m| !m.external && m.analytics.export) {
        for (name, mt) in &m.metrics {
            if mt.on.iter().any(|e| !evs.iter().any(|x| x.name == e)) {
                continue;
            }
            let cols: Vec<String> = std::iter::once("bucket".to_string())
                .chain(mt.by.iter().map(|b| snake(b)))
                .chain(std::iter::once("value".to_string()))
                .collect();
            cards.push(serde_json::json!({
                "name": format!("{name} · {} per {}", mt.kind, mt.window),
                "collection": m.service,
                "declared_in": m.service,
                "kind": "metric",
                "view": Metric::view(name),
                "sql": format!(
                    "SELECT {} FROM {dataset}.{} ORDER BY bucket",
                    cols.join(", "),
                    Metric::view(name)
                ),
            }));
        }
    }
    // One per funnel, and only for the funnels that really got a view. A card
    // pointing at a view nobody created is a dashboard that answers with an
    // error, which is worse than not being there: it reads as the data being
    // broken instead of the question being invented.
    let emitted = funnels(ms, &evs, &d).join("\n");
    for root in &evs {
        let view = format!("funnel_{}", table(root.name));
        if !emitted.contains(&view) {
            continue;
        }
        cards.push(serde_json::json!({
            "name": format!("funnel from {}", root.name),
            "collection": root.duenio,
            "declared_in": root.duenio,
            "kind": "funnel",
            "view": view,
            "sql": format!(
                "SELECT * FROM {dataset}.{view} ORDER BY step_1_{} DESC LIMIT 500",
                table(root.name)
            ),
        }));
    }
    serde_json::json!({
        "note": "Generated by axon. Apply it against a Metabase; do not edit. \
                 Every question points at a view the manifest declares, so the \
                 definition of a metric lives in one place.",
        "database": {
            "name": format!("axon warehouse ({})", d.name),
            "engine": d.name,
            "details": {
                // the names of the local target: elsewhere they are the ones of
                // whoever deploys, and that is why this is emitted and not applied
                "host": "warehouse",
                "port": 8123,
                "user": "local",
                "password": "local",
                "dbname": dataset,
                "ssl": false,
            },
        },
        "cards": cards,
    })
}

/// Moves the levers a rule decided, and writes down that it did.
///
/// Two locks and not one: the manifest says the rule MAY be applied
/// (`mode = "apply"`) and whoever runs it says apply now (`--apply`). Neither
/// on its own does anything, because the two answer different questions —is
/// this rule allowed to act, and is now the moment— and a single switch would
/// conflate them.
///
/// What it writes is the flagd configuration axon itself generated. Any other
/// flag store is somebody else's API and somebody else's credential, which is
/// the same line every other command here draws.
///
/// The audit trail is not a nicety: an automated change to production that
/// leaves no record is the worst possible version of this, and the one line it
/// appends is what somebody reads at 3am when the lever is somewhere nobody
/// remembers putting it.
/// The columns of everything axon owns in the warehouse, read from the DDL it
/// generates and not from a second list. A list kept in step by hand drifts on
/// the first change, and what drifts here is the answer to "does this column
/// exist".
fn owned(ms: &[Manifest], d: &Dialect, dataset: &str) -> IndexMap<String, Vec<String>> {
    use sqlparser::ast::{CreateTable, CreateView, Statement};
    let mut out: IndexMap<String, Vec<String>> = IndexMap::new();
    // `TTL` and `ALTER … MODIFY TTL` are valid ClickHouse and the parser does
    // not know them: a warehouse extension, not a mistake. The TTL line becomes
    // the `;` it carried —it is the last line of the CREATE, and without it the
    // statements stop being separable— and the ALTER goes away whole. What is
    // read here is the column list; the retention is checked in the table
    // itself, elsewhere.
    let ddl: String = build(ms, d)
        .replace("@dataset", dataset)
        .lines()
        .filter(|l| !l.trim_start().starts_with("ALTER TABLE"))
        .map(|l| match l.trim_start().starts_with("TTL ") {
            true => ";",
            false => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let dialect = sqlparser::dialect::ClickHouseDialect {};
    let name_of = |o: &sqlparser::ast::ObjectName| -> String {
        o.0.last()
            .map(|p| p.to_string().trim_matches(['"', '`']).to_string())
            .unwrap_or_default()
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_lowercase()
    };
    for stmt in ddl.split(";\n") {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        let Ok(parsed) = sqlparser::parser::Parser::parse_sql(&dialect, stmt) else {
            continue;
        };
        for s in parsed {
            match s {
                Statement::CreateTable(CreateTable { name, columns, .. }) => {
                    out.insert(
                        name_of(&name),
                        columns.iter().map(|c| c.name.to_string()).collect(),
                    );
                }
                // A view's columns are the aliases of its projection: that is
                // what the dashboard sees, and the only place it is written.
                Statement::CreateView(CreateView { name, query, .. }) => {
                    let mut cols = Vec::new();
                    if let sqlparser::ast::SetExpr::Select(sel) = &*query.body {
                        for item in &sel.projection {
                            match item {
                                sqlparser::ast::SelectItem::ExprWithAlias { alias, .. } => {
                                    cols.push(alias.to_string())
                                }
                                sqlparser::ast::SelectItem::UnnamedExpr(
                                    sqlparser::ast::Expr::Identifier(i),
                                ) => cols.push(i.to_string()),
                                sqlparser::ast::SelectItem::UnnamedExpr(
                                    sqlparser::ast::Expr::CompoundIdentifier(p),
                                ) => {
                                    if let Some(last) = p.last() {
                                        cols.push(last.to_string())
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    out.insert(name_of(&name), cols);
                }
                _ => {}
            }
        }
    }
    out
}

/// A question written by hand, reduced to what can be checked: which tables it
/// reads and which columns it names.
struct Question {
    name: String,
    native: bool,
    sql: String,
}

/// Reads what came back from a Metabase. Two shapes arrive here: the one axon
/// itself emits and the one `/api/card` returns, and telling the user to
/// convert between them would be asking them to do by hand exactly what this
/// exists to avoid.
fn questions(v: &serde_json::Value) -> Vec<Question> {
    let list = match v.get("cards").and_then(|c| c.as_array()) {
        Some(c) => c.clone(),
        None => v.as_array().cloned().unwrap_or_default(),
    };
    list.iter()
        .map(|c| {
            let name = c
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("(no name)")
                .to_string();
            // axon's own shape carries the SQL straight; Metabase's wraps it,
            // and an MBQL question carries no SQL at all.
            let q = c.get("dataset_query");
            let kind = q
                .and_then(|q| q.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or("native");
            let sql = c
                .get("sql")
                .and_then(|s| s.as_str())
                .or_else(|| q?.get("native")?.get("query")?.as_str())
                .unwrap_or_default()
                .to_string();
            Question {
                name,
                native: kind == "native" && !sql.is_empty(),
                sql,
            }
        })
        .collect()
}

/// What a question reads: the tables it comes FROM, and the columns it names
/// for each one. Read from the token stream, so a word inside a string literal
/// is not mistaken for a column — a rule with false positives gets silenced
/// along with everything else it says.
///
/// Returns `None` when it will not answer: a CTE, a join or a subquery makes a
/// bare name ambiguous from here, and reporting an ambiguous name as missing is
/// exactly how this stops being read.
fn reads(sql: &str) -> Option<(String, Vec<String>)> {
    use sqlparser::keywords::Keyword;
    use sqlparser::tokenizer::{Token, Tokenizer};
    let dialect = sqlparser::dialect::ClickHouseDialect {};
    let raw = Tokenizer::new(&dialect, sql).tokenize().ok()?;
    let toks: Vec<&Token> = raw
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    let (mut tables, mut cols) = (Vec::new(), Vec::new());
    let mut i = 0;
    while i < toks.len() {
        let Token::Word(w) = toks[i] else {
            i += 1;
            continue;
        };
        match w.keyword {
            // Anything that makes a bare name ambiguous: it does not answer
            Keyword::WITH | Keyword::JOIN => return None,
            Keyword::FROM => {
                // `axon.order_placed_v1` is three tokens, and the table is the
                // last one; a subquery opens with a parenthesis instead
                let mut j = i + 1;
                let mut last = None;
                while let Some(Token::Word(t)) = toks.get(j) {
                    last = Some(t.value.to_lowercase());
                    match toks.get(j + 1) {
                        Some(Token::Period) => j += 2,
                        _ => break,
                    }
                }
                // no name after FROM: a subquery, and it does not answer
                tables.push(last?);
                i = j + 1;
                continue;
            }
            // the name after AS is being defined, not read
            Keyword::AS => i += 2,
            Keyword::NoKeyword => {
                let call = matches!(toks.get(i + 1), Some(Token::LParen));
                let qualifier = matches!(toks.get(i + 1), Some(Token::Period));
                if !call && !qualifier {
                    cols.push(w.value.to_lowercase());
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    match tables.len() {
        1 => Some((tables.remove(0), cols)),
        _ => None,
    }
}

/// The other direction of the drift. axon emits the questions and compares
/// them against the manifest; a question somebody writes BY HAND in the
/// dashboard, against a table axon owns, was invisible to it. The day the
/// column changes that question breaks, and nothing says so until somebody
/// opens it.
///
/// No credentials here either: whoever has the Metabase exports its questions
/// and the compiler crosses them against what it generates.
pub fn metabase_review(
    ms: &[Manifest],
    dataset: &str,
    exported: &str,
) -> Result<(Vec<String>, Vec<String>, String), String> {
    let v: serde_json::Value =
        serde_json::from_str(exported).map_err(|e| format!("the export is not JSON: {e}"))?;
    let d = dialect("clickhouse").expect("clickhouse is a native dialect");
    let owned = owned(ms, &d, dataset);
    let mine: Vec<String> = metabase(ms, dataset)["cards"]
        .as_array()
        .map(|c| {
            c.iter()
                .filter_map(|c| Some(c.get("name")?.as_str()?.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let (mut errors, mut warnings, mut out) = (Vec::new(), Vec::new(), Vec::new());
    let qs = questions(&v);
    if qs.is_empty() {
        return Err(
            "the export carries no questions. Comparing against an empty file gives 0 \
             differences and that reads as everything being fine"
                .into(),
        );
    }
    let (mut hand, mut unchecked) = (0, 0);
    // One line each and not one per question: a Metabase ships with dozens of
    // example questions of its own, and forty warnings about somebody else's
    // sample database is how a rule stops being read.
    let (mut mbql, mut ambiguous): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    for q in &qs {
        if mine.contains(&q.name) {
            continue; // axon's own: it is generated from the manifest
        }
        hand += 1;
        if !q.native {
            unchecked += 1;
            mbql.push(q.name.clone());
            continue;
        }
        let Some((tabla, cols)) = reads(&q.sql) else {
            unchecked += 1;
            ambiguous.push(q.name.clone());
            continue;
        };
        let Some(declared) = owned.get(&tabla) else {
            out.push(format!("  {} reads `{tabla}`, which is not axon's", q.name));
            continue;
        };
        let mut missing: Vec<String> = Vec::new();
        for c in &cols {
            if declared.iter().any(|d| d.eq_ignore_ascii_case(c)) || missing.contains(c) {
                continue;
            }
            missing.push(c.clone());
        }
        out.push(format!(
            "  {}  ·  {tabla}  ·  {}",
            q.name,
            match missing.is_empty() {
                true => "ok".to_string(),
                false => format!("reads {}", missing.join(", ")),
            }
        ));
        for c in missing {
            // The commonest case is not a typo: it is the PII column, which the
            // manifest says travels hashed and therefore never exists by that
            // name. Saying which one is there turns the error into the fix.
            let near = declared
                .iter()
                .find(|d| d.starts_with(&c) || c.starts_with(d.as_str()));
            errors.push(format!(
                "`{}`: reads `{tabla}.{c}`, and that column is not in what the manifest \
                 generates{}. The question answers with an error and nothing says so until \
                 somebody opens it",
                q.name,
                match near {
                    Some(n) => format!(" —what is there is `{n}`"),
                    None => String::new(),
                }
            ));
        }
    }
    let some = |v: &[String]| match v.len() {
        0..=3 => v.join(", "),
        _ => format!("{}, and {} more", v[..3].join(", "), v.len() - 3),
    };
    if !mbql.is_empty() {
        warnings.push(format!(
            "{} question(s) are not native and are NOT checked: they name their table and \
             their fields by numeric id, and from outside the Metabase those ids say nothing \
             ({})",
            mbql.len(),
            some(&mbql)
        ));
    }
    if !ambiguous.is_empty() {
        warnings.push(format!(
            "{} question(s) join, or read a subquery or a CTE, and are NOT checked: a bare \
             name is ambiguous from here and reporting it as missing would be wrong ({})",
            ambiguous.len(),
            some(&ambiguous)
        ));
    }
    out.insert(
        0,
        format!(
            "{} question(s) exported  ·  {} generated by axon  ·  {hand} written by hand, \
             {unchecked} of them not checkable",
            qs.len(),
            qs.len() - hand
        ),
    );
    Ok((errors, warnings, out.join("\n")))
}

pub fn apply(
    proposals: &[Proposal],
    flags: &mut serde_json::Value,
    when: &str,
) -> (Vec<String>, Vec<String>) {
    let mut done = Vec::new();
    let mut audit = Vec::new();
    for p in proposals.iter().filter(|p| p.apply) {
        let Some((flag, variant, restore)) = &p.flag else {
            continue;
        };
        // Fires -> the lever goes to the declared variant. Lifted -> it goes
        // back. In between —holding, or not enough history— nothing moves: a
        // rule that rewrites the same value every window is a rule that fills
        // the audit trail with nothing.
        let target = if p.fires {
            variant
        } else if p.lifted {
            restore
        } else {
            continue;
        };
        let Some(entry) = flags.pointer_mut(&format!("/flags/{flag}")) else {
            done.push(format!(
                "`{flag}` is not in this flagd configuration: nothing was moved"
            ));
            continue;
        };
        let before = entry
            .get("defaultVariant")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        if before == *target {
            continue;
        }
        // A variant that does not exist would leave flagd serving nothing.
        // `verify` refuses it in the manifest; here the file could be older.
        if entry
            .get("variants")
            .and_then(|v| v.as_object())
            .is_none_or(|v| !v.contains_key(target.as_str()))
        {
            done.push(format!(
                "`{flag}` has no variant `{target}` in this file: nothing was moved"
            ));
            continue;
        }
        entry["defaultVariant"] = serde_json::json!(target);
        done.push(format!(
            "{}.{}: `{flag}` {before} -> {target}",
            p.service, p.rule
        ));
        audit.push(
            serde_json::json!({
                "at": when,
                "rule": p.rule,
                "service": p.service,
                "flag": flag,
                "from": before,
                "to": target,
                "why": p.why,
            })
            .to_string(),
        );
    }
    (done, audit)
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
        tail: |_| String::new(),
        diff_ms: |a, b| format!("{a} - {b}"),
        bucket: |w, c| format!("bucket({w}, {c})"),
        lag: |c, n| format!("lag({c}, {n})"),
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
        // The metrics go in the plan with axon's vocabulary —the aggregation, the
        // field as the contract names it, the bucket— and not as SQL: whoever
        // renders the plan writes the query their warehouse speaks.
        "metrics": ms.iter().filter(|m| !m.external && m.analytics.export).flat_map(|m| {
            m.metrics.iter().map(move |(name, mt)| serde_json::json!({
                "name": name,
                "owner": m.service,
                "view": Metric::view(name),
                "on": mt.on,
                "kind": mt.kind,
                "field": mt.field,
                "by": mt.by,
                "window": mt.window,
            }))
        }).collect::<Vec<_>>(),
    })
}
