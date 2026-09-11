#!/bin/sh
# The warehouse, measured: that the generated schema accepts the real events and
# that the declared funnel counts the flow that actually happened.
#
# This is what was missing: the schema was generated for three dialects and only
# GCP had an ingest path, so it could be applied on the others and left with
# empty tables with nothing warning about it.
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
COMPOSE="docker compose -f axon.local.yml"

ch() { $COMPOSE exec -T warehouse clickhouse-client --user local --password local "$@"; }

echo "  applying the generated schema"
# The schema comes out with `@dataset` as a parameter; locally the database is `axon`.
"$AXON" analytics . --target clickhouse | sed 's/"@dataset"\./axon./g' > .axon/warehouse.sql
# A `sed` that matches nothing is silent: the `@dataset` survives, ClickHouse
# answers with a syntax error twenty lines further down, and what it names is
# not what broke. It broke here.
for f in .axon/warehouse.sql .axon/rules.sql; do
  [ -f "$f" ] || continue
  ! grep -q '@dataset' "$f" || { echo "  FAILED: \`@dataset\` survived the substitution in $f"; exit 1; }
done
ch --multiquery < .axon/warehouse.sql

echo "  loading the envelope log"
# The log is written by the local target itself. The path is the one ClickHouse sees.
"$AXON" analytics . --load local.ndjson --dataset axon > .axon/load.sql
ch --param_salt=demo-salt --multiquery < .axon/load.sql

rows=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
[ "$rows" -ge 1 ] || { echo "  FAILED: the schema applied and the warehouse stayed empty"; exit 1; }
echo "  OK: $rows events in the warehouse, with the generated schema untouched"

# --- the funnel counts the real flow --------------------------------------
# The declared causal chain says an `order.placed@v1` leads to a
# `payment.captured@v1`. The funnel comes out of that declaration, so if the
# system does what it declares, the conversion is 1.
echo "  the declared funnel against the real flow"
# Only the flows that STARTED: the view groups by correlationId over the union of
# the two tables, so without this filter it also counts flows that only have step
# 2 —from an earlier run, or from a charge that was not born of an order— and
# that is no longer this funnel's conversion.
funnel=$(ch -q "SELECT count(*), countIf(step_2_payment_captured_v1 IS NOT NULL)
                  FROM axon.funnel_order_placed_v1
                 WHERE step_1_order_placed_v1 IS NOT NULL FORMAT TSV")
flows=$(printf '%s' "$funnel" | cut -f1)
converted=$(printf '%s' "$funnel" | cut -f2)
if [ "$flows" -ge 1 ] && [ "$flows" = "$converted" ]; then
  echo "  OK: $flows flows, $converted reached the charge (100% conversion)"
else
  echo "  FAILED: $flows flows and only $converted converted"
  exit 1
fi

# and the business latency is a number, not a promise
ms=$(ch -q "SELECT round(avg(ms_to_payment_captured_v1)) FROM axon.funnel_order_placed_v1
             WHERE ms_to_payment_captured_v1 IS NOT NULL")
# and it matches what is in step 1's table: if not, the funnel is counting flows
# from somewhere else
[ "$flows" = "$rows" ] || { echo "  FAILED: $flows flows in the funnel and $rows orders in the table"; exit 1; }
echo "  i the funnel's business latency: ${ms}ms from the order to the charge"

# --- the declared metrics answer what a direct query answers -------------
# A metric is a view over the same tables, so what is checked is that it says the
# same as counting by hand. If they differed, the view would be answering
# something other than what it claims — and a number in a dashboard has nobody to
# contradict it.
echo "  the declared metrics against a direct count"
placed=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
metric_count=$(ch -q "SELECT sum(value) FROM axon.metric_orders_placed")
gmv_direct=$(ch -q "SELECT sum(total_amount) FROM axon.order_placed_v1")
gmv_metric=$(ch -q "SELECT sum(value) FROM axon.metric_gmv")
# A sum over an all-NULL column returns NULL in ClickHouse, and `NULL = NULL`
# would pass this check while the metric answered nothing: the amount has to be a
# number, and a positive one.
case "$gmv_metric" in ''|*[!0-9]*) echo "  FAILED: the metric answers '$gmv_metric', not a number"; exit 1 ;; esac
if [ "$placed" = "$metric_count" ] && [ "$gmv_direct" = "$gmv_metric" ] && [ "$gmv_metric" -gt 0 ]; then
  echo "  OK: $metric_count orders and $gmv_metric cents, the same as counting the table by hand"
else
  echo "  FAILED: count $placed vs $metric_count, gmv $gmv_direct vs $gmv_metric"
  exit 1
fi

# And the bucket exists: without it a metric is one number for all of history,
# which is a total and not a metric.
buckets=$(ch -q "SELECT count(DISTINCT bucket) FROM axon.metric_gmv")
dims=$(ch -q "SELECT count(DISTINCT total_currency) FROM axon.metric_gmv")
if [ "$buckets" -ge 1 ] && [ "$dims" -ge 1 ]; then
  echo "  OK: $buckets bucket(s) and $dims currency; the metric groups by what it declares"
else
  echo "  FAILED: buckets=$buckets dimensions=$dims"
  exit 1
fi

# --- the personal data does not travel in plaintext ----------------------
# `pii = "hash"` in the manifest. That the column is a hash and not the address
# is the only way to know the policy was applied.
# La retencion, que es lo unico que decide si esta tabla existe en dos anios. Se
# lee del propio ClickHouse y se compara contra el manifiesto: no basta con que
# el DDL la diga, tiene que estar EN la tabla.
echo "  la retencion declarada, aplicada en la tabla"
# La linea de la excepcion y no la del bloque [emits]: las dos nombran el evento
declarada=$(grep '^"order.placed@v1" = ' orders.toml | sed 's/.*= //')
real=$(ch -q "SELECT extract(engine_full, 'toIntervalDay\\(([0-9]+)\\)') FROM system.tables WHERE database = 'axon' AND name = 'order_placed_v1'")
otra=$(ch -q "SELECT extract(engine_full, 'toIntervalDay\\(([0-9]+)\\)') FROM system.tables WHERE database = 'axon' AND name = 'payment_captured_v1'")
echo "    order.placed@v1 $real dias  ·  payment.captured@v1 $otra dias"
if [ "$real" = "$declarada" ] && [ -n "$otra" ] && [ "$otra" != "$real" ]; then
  echo "  OK: la excepcion por evento manda sobre la del servicio, y esta EN la tabla"
  echo "  i sin el ALTER, un IF NOT EXISTS habria ignorado la retencion anadida despues"
else
  echo "  FALLO: declarada=$declarada real=$real otra=$otra"
  exit 1
fi

echo "  the personal field, as the manifest declares it"
raw=$(ch -q "SELECT count(*) FROM axon.order_placed_v1 WHERE customer_email_hash LIKE '%@%'")
hashes=$(ch -q "SELECT count(*) FROM axon.order_placed_v1
                 WHERE length(customer_email_hash) = 64 AND customer_email_hash NOT LIKE '%@%'")
if [ "$raw" -eq 0 ] && [ "$hashes" -ge 1 ]; then
  echo "  OK: $hashes hashed, 0 addresses in plaintext"
else
  echo "  FAILED: raw=$raw hashed=$hashes"
  exit 1
fi

# --- the real schema against the declared one ----------------------------
# A new field in an event changes the generated schema, and the table that
# already exists stays as it was: the field loads as nothing and the queries keep
# returning numbers. Nobody sees an error.
echo "  the warehouse's schema against the manifest"
"$AXON" analytics . --target clickhouse --introspect | grep -v '^--' > .axon/schema.sql
ch --multiquery < .axon/schema.sql > .axon/schema.tsv
"$AXON" analytics . --target clickhouse --check .axon/schema.tsv

# and that the check WORKS: the warehouse gets broken on purpose and it has to
# see it. A check that has only been seen passing has not been seen working.
echo "  the same check, with the warehouse broken on purpose"
# The table gets copied first. Re-adding a dropped column brings it back EMPTY,
# and the loader is idempotent by event id, so the rows loaded earlier would keep
# a NULL forever. Reloading from the log does not fix it either: the log holds
# THIS run's flow, and the table accumulates every run —running the demo twice is
# what showed it, with the warehouse reporting 2 events and the idempotency check
# counting 1 row afterwards.
before_break=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
cols=$(ch -q "SELECT arrayStringConcat(groupArray(name), ', ') FROM system.columns
               WHERE database = 'axon' AND table = 'order_placed_v1'")
ch -q "DROP TABLE IF EXISTS axon.order_placed_v1_bak"
ch -q "CREATE TABLE axon.order_placed_v1_bak AS axon.order_placed_v1"
ch -q "INSERT INTO axon.order_placed_v1_bak ($cols) SELECT $cols FROM axon.order_placed_v1"

ch -q "ALTER TABLE axon.order_placed_v1 DROP COLUMN total_amount"
ch --multiquery < .axon/schema.sql > .axon/broken.tsv
if "$AXON" analytics . --target clickhouse --check .axon/broken.tsv > /dev/null 2>&1; then
  echo "  FAILED: a declared column is missing and the check passed"
  ch -q "ALTER TABLE axon.order_placed_v1 ADD COLUMN total_amount Nullable(Int64)"
  exit 1
fi
echo "  OK: the missing column is detected, and without it that field would be stored nowhere"

# Restored from the copy, naming the columns: the re-added one comes back at the
# END of the table, so an `INSERT ... SELECT *` would load the amount into the
# currency. That is the same bug the loader had, and it is why both name them.
ch -q "ALTER TABLE axon.order_placed_v1 ADD COLUMN total_amount Nullable(Int64)"
ch -q "TRUNCATE TABLE axon.order_placed_v1"
ch -q "INSERT INTO axon.order_placed_v1 ($cols) SELECT $cols FROM axon.order_placed_v1_bak"
ch -q "DROP TABLE axon.order_placed_v1_bak"
after_break=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
amounts=$(ch -q "SELECT count(*) FROM axon.order_placed_v1 WHERE total_amount > 0")
if [ "$after_break" = "$before_break" ] && [ "$amounts" = "$before_break" ]; then
  echo "  OK: restored whole after breaking it: $after_break rows, all with their amount"
else
  echo "  FAILED: $before_break rows before, $after_break after, $amounts with an amount"
  exit 1
fi

# --- loading twice does not duplicate ------------------------------------
# A periodic loader runs many times over the same log. If it does not filter by
# what is already loaded, every event multiplies and the funnel lies without
# failing.
echo "  the loader, run twice"
before=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
ch --param_salt=demo-salt --multiquery < .axon/load.sql
after=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
if [ "$before" = "$after" ]; then
  echo "  OK: $before rows before and after; the loader is idempotent"
else
  echo "  FAILED: $before -> $after, the loader duplicates"
  exit 1
fi
