#!/bin/sh
# The k8s ingest path, MEASURED. Until now the generated Vector config was only
# run through `vector validate`: valid does not mean it carries an event from
# the broker to the warehouse, and least of all that it hashes what it has to.
#
# The generated file names the containers this same target brings up
# (`nats://broker:4222`, `http://warehouse:8123`), so what runs here is the
# config as it is, with nothing rewritten.
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
COMPOSE="docker compose -f axon.local.yml"
SALT=demo-salt
EMAIL=vector@demo.mx
ID=vector-$(date +%s)

ch() { $COMPOSE exec -T warehouse clickhouse-client --user local --password local "$@"; }

# The image comes out of the generator, not out of this script: two places
# naming a version drift apart, and what would be measured then is another
# Vector than the one the k8s manifest deploys.
IMG=$("$AXON" infra . --target k8s | sed -n 's/.*image: \(timberio\/vector:[^ ]*\).*/\1/p' | head -1)
[ -n "$IMG" ] || { echo "  FAILED: the k8s target no longer names a Vector image"; exit 1; }
NET=$(docker inspect -f '{{range $k,$v := .NetworkSettings.Networks}}{{$k}}{{end}}' \
        "$($COMPOSE ps -q broker)")

"$AXON" analytics . --vector > .axon/vector.yaml

# Its own row leaves the table before and after: this event has no charge
# behind it, so leaving it there makes the NEXT run's funnel count a flow that
# did not convert. `mutations_sync=1` because the delete is asynchronous and
# without it the count is read before it happened.
wipe() {
  ch -q "ALTER TABLE axon.order_placed_v1 DELETE WHERE event_id LIKE 'vector-%' \
         SETTINGS mutations_sync = 1" 2>/dev/null || true
}
wipe

# TWO replicas on purpose. The generated file declares a queue group and says
# why: without it every replica writes the same row and the funnel counts each
# flow as many times as there are replicas. That claim is only worth something
# measured.
cleanup() { docker rm -f axon-vector-1 axon-vector-2 >/dev/null 2>&1 || true; wipe; }
trap cleanup EXIT
for n in 1 2; do
  docker run -d --rm --name "axon-vector-$n" --network "$NET" \
    -e AXON_PII_SALT="$SALT" -e AXON_WAREHOUSE_USER=local -e AXON_WAREHOUSE_PASSWORD=local \
    -v "$PWD/.axon/vector.yaml:/etc/vector/vector.yaml:ro" \
    "$IMG" --config /etc/vector/vector.yaml >/dev/null
done

# Subscribed before publishing: NATS core delivers to whoever is listening, and
# an event published into the void is not a failure of the pipeline.
i=0
while [ "$i" -lt 60 ]; do
  a=$(docker logs axon-vector-1 2>&1 | grep -c "Vector has started" || true)
  b=$(docker logs axon-vector-2 2>&1 | grep -c "Vector has started" || true)
  [ "$a" -ge 1 ] && [ "$b" -ge 1 ] && break
  i=$((i + 1)); sleep 1
done
[ "$i" -lt 60 ] || { echo "  FAILED: Vector did not start"; docker logs axon-vector-1; exit 1; }
sleep 2   # the subscription lands a moment after the log line

echo "  one real envelope, published to the broker"
docker run --rm --network "$NET" natsio/nats-box:0.14.5 \
  nats pub order.placed.v1 -s nats://broker:4222 \
  "{\"id\":\"$ID\",\"type\":\"order.placed@v1\",\"source\":\"orders\",\"time\":\"2026-09-08T12:00:00Z\",\"traceparent\":\"00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01\",\"correlationId\":\"$ID\",\"causationId\":null,\"data\":{\"orderId\":\"o-$ID\",\"customerId\":\"c-1\",\"customerEmail\":\"$EMAIL\",\"total\":{\"amount\":2500,\"currency\":\"MXN\"}}}" \
  >/dev/null 2>&1

i=0
rows=0
while [ "$i" -lt 60 ]; do
  rows=$(ch -q "SELECT count(*) FROM axon.order_placed_v1 WHERE event_id = '$ID'")
  [ "$rows" -ge 1 ] && break
  i=$((i + 1)); sleep 1
done
if [ "$rows" -ne 1 ]; then
  echo "  FAILED: $rows rows for one event (the queue group does not deduplicate, or nothing arrived)"
  docker logs axon-vector-1 2>&1 | tail -20
  exit 1
fi
echo "  OK: 1 row from 2 replicas; the queue group delivers the event once"

echo "  and the PII, which is what nobody was checking"
got=$(ch -q "SELECT customer_email_hash FROM axon.order_placed_v1 WHERE event_id = '$ID'")
# The other path into the same table is the SQL loader, and it hashes with its
# own expression. Two ingests with different hashes for the same person are two
# columns nobody can join, and neither of the two would look wrong on its own.
want=$(ch -q "SELECT lower(hex(SHA256(concat('$SALT', '$EMAIL'))))")
if [ "$got" != "$want" ]; then
  echo "  FAILED: Vector hashes differently from the loader"
  echo "    vector $got"
  echo "    loader $want"
  exit 1
fi
plain=$(ch -q "SELECT count(*) FROM axon.order_placed_v1 WHERE position(customer_email_hash, '@') > 0")
[ "$plain" -eq 0 ] || { echo "  FAILED: $plain rows carry an address in plaintext"; exit 1; }
echo "  OK: the same hash as the SQL loader, and no address reaches the warehouse"

# And what the columns are worth: the map has to line up with the schema, not
# just produce a row.
vals=$(ch -q "SELECT trace_id, total_amount, total_currency FROM axon.order_placed_v1 WHERE event_id = '$ID' FORMAT TSV")
want_vals="4bf92f3577b34da6a3ce929d0e0e4736	2500	MXN"
if [ "$vals" != "$want_vals" ]; then
  echo "  FAILED: the map does not line up with the schema"
  echo "    got  $vals"
  echo "    want $want_vals"
  exit 1
fi
echo "  OK: trace_id out of the traceparent, and the money in its two columns"
