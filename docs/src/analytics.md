# Warehouse and business metrics

This is **derivable in full**: axon already knows every event's schema, which fields are
personal, and —most importantly— **who causes whom**. That last part is the one no
warehouse has.

```toml
[analytics]
export    = true
pii       = "hash"        # or "exclude", which is the default
warehouse = "clickhouse"  # bigquery · snowflake · clickhouse
```

```sh
axon analytics manifests/ --target clickhouse > warehouse.sql
```

## The warehouse is declared, and that is why there is ingest

`warehouse` used to be a CLI flag, and that allowed generating Snowflake's schema and
deploying infrastructure that **carries nothing there**: the schema applied, the tables
stayed empty, and nobody saw an error. An empty warehouse is indistinguishable from
"nothing happened in the business".

Declared, `axon infra` can wire the ingest — or refuse:

```console
$ axon infra manifests/ --target gcp
error  `[analytics] warehouse = "clickhouse"` has no ingest path on `gcp`. The schema
       gets generated all the same and the tables would stay empty without a single
       error. Wired combinations: gcp+bigquery, aws+snowflake, aws+clickhouse,
       local+clickhouse, k8s+clickhouse. Or `export = false` if this environment does
       not export.
```

| target + warehouse | what gets deployed |
| --- | --- |
| `gcp` + `bigquery` | a Pub/Sub subscription that writes **straight** into the table, with `use_table_schema` and its own DLQ |
| `aws` + `snowflake` or `clickhouse` | one Firehose per event into S3, with the **same date partitioning** as the generated schema. From there the warehouse loads with its own tooling —Snowpipe, an external table— because that step lives on the warehouse's side, not the provider's |
| `local` + `clickhouse` | a ClickHouse and a loader for the envelope log the target itself already writes |
| `k8s` + `clickhouse` | a **Vector** that consumes from the broker and writes into the warehouse, with the config generated and validated by `vector validate` |

And one warehouse per platform: `verify` requires every exporting service to declare the
same one. Split across two, the funnel —which is what makes exporting useful— cannot be
assembled with one query, and each table would exist with rows in it with nothing warning
about it.

### Why the Firehose carries an `error_output_prefix`

What does not fit the schema lands in `errores/` and can be loaded again. It is the
warehouse's equivalent of the DLQ: an event dropped in silence is a hole in the history
nobody will notice until somebody asks about a number that does not add up.

## One table per event

```sql
CREATE TABLE IF NOT EXISTS `@dataset.order_placed_v1` (
  event_id STRING NOT NULL,
  event_type STRING NOT NULL,
  source STRING NOT NULL,
  event_time TIMESTAMP NOT NULL,
  trace_id STRING,
  correlation_id STRING NOT NULL,
  causation_id STRING,
  order_id STRING,
  customer_id STRING,
  total_amount INT64,
  total_currency STRING
)
PARTITION BY DATE(event_time)
CLUSTER BY correlation_id, source;
```

Three decisions that are not about style:

- **The envelope's columns always go.** Without `correlation_id` there is no funnel
  possible, and without `causation_id` there is no way to reconstruct what triggered what.
- **Partitioning is not optional.** Without `PARTITION BY DATE(event_time)`, every query
  scans the whole table and the bill grows with the history.
- **`money` is flattened into two columns.** You cannot sum an object, and an amount you
  cannot sum is no use in a warehouse. The names become `snake_case`: the contracts use
  the language's convention, the warehouse uses its own.

## The personal data, by explicit decision

```toml
pii = ["customerEmail"]

[analytics]
pii = "hash"        # or "exclude", which is the default
```

**The default is not to export it.** A warehouse is where personal data lives longest,
gets copied most and is read by the most people, so the safe value has to be the one that
does not send it.

With `pii = "hash"` a salted SHA-256 is exported, so unique customers can be counted
without storing the address. And `verify` warns about what that still is:

```console
warn   orders: exports 2 hashed personal fields to the warehouse. A hash is not
       anonymisation: it identifies the same person across tables, so it works for
       counting and for joining alike
```

## The funnel comes from the declared chain

This is what no other tool can do: **a funnel is normally assembled by guessing how the
events relate.** Here it is written in the manifest, so the view is derived.

```sql
CREATE OR REPLACE VIEW `@dataset.funnel_order_placed_v1` AS
SELECT
  correlation_id,
  MIN(IF(event_type = 'order.placed@v1',     event_time, NULL)) AS step_1_order_placed_v1,
  MIN(IF(event_type = 'payment.captured@v1', event_time, NULL)) AS step_2_payment_captured_v1,
  TIMESTAMP_DIFF(
    MIN(IF(event_type = 'payment.captured@v1', event_time, NULL)),
    MIN(IF(event_type = 'order.placed@v1',     event_time, NULL)),
    MILLISECOND
  ) AS ms_to_payment_captured_v1
FROM (
    SELECT correlation_id, event_type, event_time FROM `@dataset.order_placed_v1`
    UNION ALL
    SELECT correlation_id, event_type, event_time FROM `@dataset.payment_captured_v1`
)
GROUP BY correlation_id;
```

One row per business flow. **A step that is `NULL` is a flow that did not get there: that
is the conversion.** And the `TIMESTAMP_DIFF` is the *business* latency — how long an order
takes to be charged, not how long an HTTP request takes.

The steps are the same ones [`axon seq`](./cli.md) draws, because they come from the same
declaration. If tomorrow you add a consumer to the chain, the funnel gains a step with
nobody editing the SQL.

## The sink, with no process in between

The `gcp` target emits the subscription that writes **straight** into BigQuery:

```hcl
resource "google_pubsub_subscription" "order_placed_v1_warehouse" {
  topic = google_pubsub_topic.order_placed_v1.name
  bigquery_config {
    table            = "${var.project}.${var.dataset}.order_placed_v1"
    use_table_schema = true
    write_metadata   = true
  }
  dead_letter_policy { ... }
}
```

There is no process to keep, and no extra place the event could be lost. And **the
warehouse carries a DLQ too**: a message that does not fit the schema cannot disappear in
silence — which is exactly what happens when somebody changes a field and nobody looks at
the table.

## Three warehouses, one schema

```sh
axon analytics manifests/ --target bigquery|snowflake|clickhouse|plan
```

The schema and the funnels are **the same** —they come from the same manifest—; what
changes is the dialect. And the differences are not cosmetic:

| | BigQuery | Snowflake | ClickHouse |
| --- | --- | --- | --- |
| String | `STRING` | `VARCHAR` | `Nullable(String)` |
| Integer | `INT64` | `NUMBER(38,0)` | `Nullable(Int64)` |
| Timestamp | `TIMESTAMP` | `TIMESTAMP_TZ` | `Nullable(DateTime64(3))` |
| JSON | `JSON` | `VARIANT` | `String` |
| Partitioning | `PARTITION BY DATE(...)` | automatic | `PARTITION BY toYYYYMM(...)` |
| Clustering | `CLUSTER BY` | `CLUSTER BY (...)` | `ORDER BY (...)` in `MergeTree` |
| Latency | `TIMESTAMP_DIFF` | `TIMESTAMPDIFF` | `dateDiff` |

- On **Snowflake** declaring `PARTITION BY` would be an error, not an optimisation: its
  micro-partitions are automatic.
- On **ClickHouse** nullability goes in the type, and `MergeTree`'s `ORDER BY` decides
  which queries are fast — the flow goes first, because a funnel groups by it.
- The funnels use `CASE WHEN` and not `IF()`/`IFF()`: it is the only thing all three
  understand the same way.

For any other one, `--target plan` gives the neutral plan in JSON —tables, columns, axon's
types, partitioning, clustering and the PII mode— and you render it yourself.

## Verified

The suite **parses each warehouse's DDL with its own `sqlparser` dialect**, it does not
compare it against expected text. Comparing against text would have found none of the
three bugs this found:

- A comment at the end of a column **swallows the comma** separating it from the next one
  — the same mistake I had already made in a migration.
- Unwrapping `Nullable(DateTime64(3))` by stripping every trailing parenthesis left
  `DateTime64(3,` and broke the parameterised type.
- ClickHouse expects `ORDER BY` right after the engine: with `PARTITION BY` in between,
  the DDL does not parse.

## Measured in the demo, against ClickHouse

The local target brings the warehouse up and fills it from the envelope log. That is not a
demo artefact: `AXON_TRACE_LOG` is in the generated compose, so **the trace and the
analytics come from the same source** — and each row's `trace_id` comes from the
envelope's `traceparent`, so from a funnel you can jump to that specific flow's trace.

```console
==> the warehouse: schema, funnel and PII
  OK: 12 events in the warehouse, with the generated schema untouched
  OK: 12 flows, 12 reached the charge (100% conversion)
  i the funnel's business latency: 15ms from the order to the charge
  OK: 12 hashed, 0 addresses in plaintext
  OK: 12 rows before and after; the loader is idempotent
```

Five things, each one a different question:

| | |
| --- | --- |
| **the generated schema accepts the real events** | it is applied untouched. If a column or a type did not fit, the load would fail here and not in production |
| **the declared funnel counts the flow that happened** | and the funnel's flows match step 1's rows: otherwise it would be counting flows from somewhere else |
| **the business latency is a number** | from `order.placed@v1` to `payment.captured@v1`, not a request's |
| **the personal field does not travel in plaintext** | `pii = "hash"` declared → 64 hex characters, 0 at-signs. That the policy applied is not something you learn by reading the manifest |
| **loading twice does not duplicate** | a periodic loader runs many times over the same log; without filtering by what is already loaded, every event multiplies and the funnel lies without failing |

The loader comes from the same place as the schema —the columns and their paths inside the
JSON— so they cannot drift apart. And the hash's salt comes in as a parameter, never in the
generated SQL.

## Warehouse drift

Adding a field to an event changes the generated schema. The table that already exists
stays as it was: the field loads as nothing and the queries keep returning numbers.
**There is no error anywhere** — and that is why nobody looks at it.

```sh
axon analytics manifests/ --introspect > schema.sql   # the query to information_schema
# ... run it against the warehouse ...
axon analytics manifests/ --check schema.tsv          # the compiler does the diff
```

The query is **emitted** instead of run, for the same reason as `axon load --check`: axon
does not have —nor want— warehouse credentials.

| what it finds | why it matters |
| --- | --- |
| a declared column is missing | that field is stored nowhere |
| the whole table is missing | the event is emitted and stored nowhere |
| the type is not of the same family | a date stored as text **sorts wrong and raises no error** |
| the plaintext address next to its hash | the manifest says `pii = "hash"`, the new column gets filled and **the old one keeps the addresses it already had** |
| the field declared `exclude` exists in the warehouse | the personal data was left by an earlier version and does not leave on its own |
| an extra column | a warning: it breaks nothing, and it keeps being queried |

The types are compared by **family** —`Nullable(String)`, `STRING` and `text` are the same
type with three names— because what breaks a query without warning is not the type's name,
it is confusing a date with a text.

### An empty dump cannot give zero

That is the result of running the query against the wrong warehouse, and reading it as
"all fine" is worse than not checking. So a file with no columns is an **error**, not a
clean diff.

### Checked by breaking it

The demo runs the check against the real warehouse —0 differences— and then **drops a
column on purpose** and runs it again, which is the only thing that proves it works:

```console
  the warehouse's schema against the manifest
axon: the warehouse has 0 differences against the manifest
  the same check, with the warehouse broken on purpose
  OK: the missing column is detected, and without it that field would be stored nowhere
  OK: restored whole after breaking it: 12 rows, all with their amount
```

A check that has only been seen passing has not been seen working.

That last line was earned. Re-adding a dropped column brings it back EMPTY and at the
END of the table, and the loader is idempotent by event id: the rows loaded earlier keep
a NULL forever. Reloading from the envelope log does not fix it either, because the log
holds one run's flow while the table accumulates every run — running the demo twice is
what showed it, with the warehouse reporting 2 events and the idempotency check counting
1 row right afterwards. The check now copies the table before breaking it and restores it
by NAMED columns, and asserts the row count came back.

## The cluster: consume, do not subscribe

A cluster brings no managed warehouse, so there is nothing to subscribe to — that is what
left `k8s` with no path. The answer is not for axon to ship a consumer: it **generates the
configuration of a real tool**, same as with flagd, pgdog or Flyway.

```sh
axon analytics manifests/ --vector > vector.yaml
kubectl create configmap axon-warehouse --from-file=vector.yaml
```

And `axon infra --target k8s` deploys the `Deployment` that reads it.

### Four decisions, and all four came out of `vector validate`

**One source per event, not a wildcard with a router.** A `route` leaves an `_unmatched`
branch, and the event that lands there **is dropped in silence**. `vector validate` warns
about it — and a warning today is a lost event tomorrow. With one source per subject,
nothing unexpected can arrive.

**`queue: axon-warehouse`.** Without the queue group, every replica writes the same row and
the funnel counts each flow as many times as there are replicas.

**`skip_unknown_fields: false`.** A field the table does not have is an **error**, not
something to drop: it means the schema and the manifest drifted apart, and
`axon analytics --check` is what says which way.

**`buffer: { type: disk, when_full: block }`.** In memory, a restart loses what it did not
manage to write. And dropping when the buffer fills loses events exactly under the load
that makes them valuable.

### The ConfigMap is not an object, on purpose

`axon infra --target k8s` does **not** emit the ConfigMap. An empty one with that name
would overwrite the real one the first time somebody applied the file, and Vector would
come back up with nothing to read. Instead the manifest carries the exact command to
create it, and the one for the secret with the credentials — which are not there either: a
generated manifest is no place for them.

## Declared metrics

The funnel answers conversion and business latency because both are derivable from who
causes whom. A sum by dimension is not: somebody has to say which field, grouped by what,
in which bucket. Declared, it becomes a view next to the funnels instead of a query
pasted into a dashboard.

```toml
[metrics.orders_placed]
on     = ["order.placed@v1"]
kind   = "count"
window = "1d"

[metrics.gmv]
on     = ["order.placed@v1"]
kind   = "sum"
field  = "total"          # `money`, so what gets added up is its amount column
by     = ["total.currency"]
window = "1d"
```

`count`, `sum` and `avg`, and the buckets `1h`, `1d`, `1w`, `1mo` — the ones that mean the
same thing in the three warehouses. A quantile does not: it changes name per dialect, and
a metric that means something slightly different in each is worse than no metric.

```sql
CREATE OR REPLACE VIEW `@dataset.metric_gmv` AS
SELECT
  TIMESTAMP_TRUNC(event_time, DAY) AS bucket,
  total_currency,
  sum(total_amount) AS value
FROM `@dataset.order_placed_v1`
GROUP BY bucket, total_currency;
```

The field is named as the **contract** names it: `total` is `money`, so the view adds up
`total_amount`, and `total.currency` is a dimension even though the contract declares one
field. The manifest never talks about warehouse columns.

### What `verify` refutes

| | |
| --- | --- |
| A metric over an event nobody emits | the view gets applied and counts zero forever |
| A `sum` over a field that is not a number | one warehouse refuses it at apply time and another answers zero, and zero reads like nothing was sold |
| A `sum` with no `field` | there is nothing to add up |
| A dimension the event does not declare | it turns into a NULL group, and a metric with a NULL group is one nobody can read |
| **A dimension that is a `pii` field** | one row per person is not a metric, and hashing it does not change that: the hash identifies the same person across tables |
| A `kind` or a `window` that is not in the closed list | a metric that means something else in each warehouse |
| A metric while the service declares `export = false` | it reads tables that carry nothing |
| Two services declaring the same metric name | both land on the same view, and whichever applies second overwrites the first with no error |

A `count` with a `field` is a warning: the field is ignored, and nobody reading the
declaration would guess so.

### Measured against ClickHouse

The demo compares each metric against counting the table by hand. If they differed, the
view would be answering something other than what it claims — and a number in a dashboard
has nobody to contradict it.

```console
  the declared metrics against a direct count
  OK: 11 orders and 26000 cents, the same as counting the table by hand
  OK: 1 bucket(s) and 1 currency; the metric groups by what it declares
```

That check found a real bug on its first run: the loader's `INSERT ... SELECT` matched
columns **by position**, so after the drift check dropped and re-added `total_amount`
—which brings the column back at the end of the table— the amount was loaded into the
currency column and the metric answered NULL. Nothing failed. The loader now names its
columns, so a column that moved is harmless and one that is missing is an error.

## Something to read it with

A metric declared in the manifest and retyped in a dashboard is two definitions of the
same number, and the day they diverge nobody can say which one is right. That is the
failure mode a BI tool has by default, and it is the one this closes.

`axon infra --target local` brings up **Metabase** next to the warehouse — the official
image ships the ClickHouse driver, which was checked and not assumed, and it answers
`/api/health` in about fifteen seconds. It comes up empty on purpose:

```sh
axon analytics manifests/ --metabase > bi.json
```

emits what it needs — the connection, and **one question per declared metric and per
funnel that really has a view** — for somebody else to apply. axon holds no credentials
for the warehouse and none for the BI tool either, which is the same rule as
`--introspect` and `axon rules`.

Every question points at the generated view, never at a raw table. So the definition
stays in one place: the manifest declares `gmv`, the view computes it, and the dashboard
reads the view.

The demo measures exactly that agreement — it provisions a Metabase from zero without
touching the interface, creates the questions, and compares one of them against the same
view read straight from ClickHouse:

```console
==> el tablero, aprovisionado desde el manifiesto
  declarado: 4 pregunta(s) —una por metrica y por embudo— contra las vistas generadas
  Metabase aprovisionado desde cero, sin tocar la interfaz
  OK: 4 preguntas creadas, cada una apuntando a la vista que declara el manifiesto
    metabase 276300  ·  clickhouse 276300
  OK: la pregunta contesta lo mismo que la vista; la metrica se define en un solo lugar
```

The port is `AXON_BI_PORT`, and it defaults to **3030** and not Metabase's 3000: that one
is taken on any machine with a dev server running, and a demo that fails to bind reads as
the demo being broken.

What is NOT here: detecting the drift the other way — a question written by hand in
Metabase against a table axon owns. That is a parallel metric being born, and today
nobody sees it. It needs reading Metabase's API, which is one more credential, and the
same debate as the warehouse's.

## What is missing

Declarable retention for the warehouse's tables.

And `k8s`'s ingest is validated but not measured against containers: the local target is
filled from the envelope log, so Vector's path does not go through the demo. Bringing it up
there —broker, Vector and ClickHouse— is what would put it at the level of the rest.
