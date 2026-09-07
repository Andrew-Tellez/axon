#!/bin/sh
# Event sourcing and CQRS, measured against Postgres.
#
# Two claims only a real test proves:
#   * the UNIQUE (stream_id, version) IS the optimistic concurrency: two writes
#     to the same version have to leave ONE.
#   * the view is a projection, not a second source of truth: it has to match
#     what comes out of rebuilding the stream.
set -eu
cd "$(dirname "$0")"
COMPOSE="docker compose -f axon.local.yml"
CHECKOUT="localhost:${AXON_PORT_checkout:-8081}"

sql() { $COMPOSE exec -T db-checkout env PGPASSWORD=local psql -qtAX -U postgres -d checkout "$@"; }

# The demo's switches travel through `.env.local`, which is the `env_file` the
# generated compose already mounts. It is always restored, failure included.
ENVQ=.env.local
cp "$ENVQ" "$ENVQ.es"
restore_env() { mv -f "$ENVQ.es" "$ENVQ" 2>/dev/null || true; }
trap 'restore_env' EXIT

echo "  one checkout: the stream, not a row"
r=$(curl -sS --fail-with-body -m 60 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d '{"orderId":"11111111-1111-4111-8111-111111111111","amount":{"amount":2500,"currency":"MXN"}}')
echo "    $r"
STREAM=$(sql -c "SELECT stream_id FROM checkout_event ORDER BY at DESC LIMIT 1" | tr -d ' \r\n')
events=$(sql -c "SELECT string_agg(type, ' -> ' ORDER BY version) FROM checkout_event WHERE stream_id = '$STREAM'")
echo "    $events"
[ -n "$events" ] || { echo "  FAILED: the stream came out empty"; exit 1; }

# --- optimistic concurrency, measured ------------------------------------
# Two INSERTs at the SAME version, at the same time. Without the UNIQUE both get
# in, nobody sees an error, and the rebuilt state depends on the read order.
echo "  two concurrent writes at the same version"
last=$(sql -c "SELECT max(version) FROM checkout_event WHERE stream_id = '$STREAM'" | tr -d ' \r\n')
next=$((last + 1))
one=/tmp/axon-es-1.log
two=/tmp/axon-es-2.log
ins="INSERT INTO checkout_event (id, stream_id, version, type, data)
     VALUES (gen_random_uuid(), '$STREAM', $next, 'checkout.compensated@v1', '{\"streamId\":\"$STREAM\",\"reason\":\"race\"}'::jsonb)"
# the `pg_sleep` inside the transaction overlaps them on purpose
sql -v ON_ERROR_STOP=1 -c "BEGIN; SELECT pg_sleep(0.4); $ins; COMMIT;" > "$one" 2>&1 &
p1=$!
sql -v ON_ERROR_STOP=1 -c "BEGIN; SELECT pg_sleep(0.4); $ins; COMMIT;" > "$two" 2>&1 &
p2=$!
ok=0
wait $p1 && ok=$((ok + 1)) || true
wait $p2 && ok=$((ok + 1)) || true
rows=$(sql -c "SELECT count(*) FROM checkout_event WHERE stream_id = '$STREAM' AND version = $next" | tr -d ' \r\n')
if [ "$ok" -eq 1 ] && [ "$rows" -eq 1 ]; then
  echo "  OK: one got in and the other was rejected; 1 event left at version $next"
  grep -q "duplicate key" "$two" "$one" && echo "  i the rejection is the UNIQUE, not a check in the application" || true
else
  echo "  FAILED: $ok successful writes and $rows rows at version $next"
  cat "$one" "$two"
  exit 1
fi

# --- the view agrees with the stream -------------------------------------
# It is the only way to know the projection did not stop halfway: if the view and
# the fold say different things, the read model is lying.
echo "  the view against the state rebuilt from the stream"
# the stream's last event rules: the state the view should be showing
expected=$(sql -c "SELECT replace(split_part(type, '.', 2), '@v1', '')
                     FROM checkout_event WHERE stream_id = '$STREAM'
                    ORDER BY version DESC LIMIT 1" | tr -d ' \r\n')
in_view=$(sql -c "SELECT state FROM view_conversion WHERE stream_id = '$STREAM'" | tr -d ' \r\n')
# the race above recorded an event the view did not see: it gets reprojected
position=$(sql -c "SELECT coalesce(position, 0) FROM view_conversion_checkpoint
                    WHERE view_name = 'conversion' AND stream_id = '$STREAM'" | tr -d ' \r\n')
echo "    the stream says '$expected', the view says '$in_view', checkpoint at $position"
if [ "$expected" = "$in_view" ]; then
  echo "  OK: the view agrees with the stream"
else
  # Not a failure of the system: the race above wrote DIRECTLY to the stream,
  # bypassing the projection. What matters is that the checkpoint says so.
  last=$(sql -c "SELECT max(version) FROM checkout_event WHERE stream_id = '$STREAM'" | tr -d ' \r\n')
  if [ "${position:-0}" -lt "$last" ]; then
    echo "  OK: the view is behind AND the checkpoint says so ($position of $last)"
    echo "  i an event written to the stream without going through the projection"
    echo "    leaves the view behind, and the checkpoint is the only thing that lets"
    echo "    you know and resume"
  else
    echo "  FAILED: the view says '$in_view' and the checkpoint is up to date at $position"
    exit 1
  fi
fi

# --- the lag, against the declared budget --------------------------------
# A view's lag is NOT the age of the event it already applied: it is how long it
# has gone without seeing what already happened. Measuring it from the view
# always gives a pretty number —precisely when it is stopped— so it comes from
# the STREAM: the age of the oldest event the projection has not applied yet.
echo "  the view's lag against its budget"
budget=$(sed -n 's/.*conversionMaxStalenessMs = \([0-9]*\).*/\1/p' services/checkout/contracts.ts | head -1)
# The checkpoint is PER STREAM: an event's version is its position inside its
# stream, so a single number for the whole view identifies nothing as soon as
# there is more than one stream. With one it seemed to work.
lag() {
  sql -c "SELECT coalesce(max(extract(epoch from (now() - e.at)) * 1000)::bigint, 0)
            FROM checkout_event e
            LEFT JOIN view_conversion_checkpoint c
              ON c.view_name = 'conversion' AND c.stream_id = e.stream_id
           WHERE e.stream_id = '$1' AND e.version > coalesce(c.position, 0)" | tr -d ' \r\n'
}

# The stream above was left with an event NOT projected —the race wrote it,
# bypassing the projection— so the measurement has to see it.
behind=$(lag "$STREAM")
if [ "$behind" -gt 0 ]; then
  echo "  OK: ${behind}ms of real lag; the unprojected event shows up"
else
  echo "  FAILED: there is an unprojected event and the measured lag is 0"
  exit 1
fi

# And a new checkout, with the view up to date, has to fit in the budget.
# Without this second case, the measurement could be always giving a high number.
echo "  and a checkout that is up to date, inside the budget"
curl -sS --fail-with-body -m 60 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d '{"orderId":"44444444-4444-4444-8444-444444444444","amount":{"amount":900,"currency":"MXN"}}' > /dev/null
FRESH=$(sql -c "SELECT stream_id FROM checkout_event ORDER BY at DESC LIMIT 1" | tr -d ' \r\n')
current=$(lag "$FRESH")
if [ "$current" -le "$budget" ]; then
  echo "  OK: ${current}ms, inside the declared budget of ${budget}ms"
else
  echo "  FAILED: ${current}ms against a budget of ${budget}ms"
  exit 1
fi

# --- the relay: nobody publishes inline ----------------------------------
# The stream is the truth and the outbox is the delivery, written in ONE
# transaction. What gets measured is what that buys: an event recorded while the
# relay is down gets published when it comes back, with nobody retrying by hand.
echo "  an event recorded with the relay down"
$COMPOSE stop checkout > /dev/null 2>&1
ORPHAN=$(sql -c "SELECT gen_random_uuid()" | tr -d ' \r\n')
last=$(sql -c "SELECT max(version) FROM checkout_event WHERE stream_id = '$FRESH'" | tr -d ' \r\n')
# Both rows, in one transaction, exactly as `append` does.
sql -v ON_ERROR_STOP=1 -c "BEGIN;
  INSERT INTO checkout_event (id, stream_id, version, type, data)
  VALUES ('$ORPHAN', '$FRESH', $((last + 1)), 'checkout.compensated@v1',
          '{\"streamId\":\"$FRESH\",\"reason\":\"relay down\"}'::jsonb);
  INSERT INTO outbox (id, type, source, time, traceparent, correlation_id, causation_id, data)
  VALUES ('$ORPHAN', 'checkout.compensated@v1', 'checkout', now()::text,
          '00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01', '$FRESH', NULL,
          '{\"streamId\":\"$FRESH\",\"reason\":\"relay down\"}'::jsonb);
  COMMIT;" > /dev/null
pending=$(sql -c "SELECT count(*) FROM outbox WHERE published_at IS NULL" | tr -d ' \r\n')
[ "$pending" -ge 1 ] || { echo "  FAILED: the event was not left pending"; $COMPOSE up -d --wait checkout > /dev/null 2>&1; exit 1; }
echo "    $pending event(s) recorded and unpublished"

$COMPOSE up -d --wait checkout > /dev/null 2>&1
i=0
while [ "$(sql -c "SELECT count(*) FROM outbox WHERE id = '$ORPHAN' AND published_at IS NOT NULL" | tr -d ' \r\n')" -eq 0 ]; do
  i=$((i + 1))
  [ "$i" -gt 30 ] && { echo "  FAILED: the relay came back and did not publish what was pending"; exit 1; }
  sleep 1
done
# and it really reached the bus, it was not just marked
if grep -q "$ORPHAN" .axon/local.ndjson 2>/dev/null; then
  echo "  OK: the relay came back and published it; nobody had to retry by hand"
else
  echo "  FAILED: it was marked as published and does not show up in the envelope log"
  exit 1
fi

# --- the snapshots: a cache that cannot lie ------------------------------
# A snapshot is a cache of the fold. The dangerous part is not it being missing
# —that only costs time— but it being WRONG: rehydrating from an incorrect
# snapshot gives a state that no longer matches replaying the stream, and that
# raises no error at all.
echo "  the snapshot, at the declared cadence"
every=$(sed -n 's/.*checkoutSnapshotEvery = \([0-9]*\).*/\1/p' services/checkout/contracts.ts | head -1)
rules=$(sed -n 's/.*checkoutSnapshotRules = \([0-9]*\).*/\1/p' services/checkout/contracts.ts | head -1)
SNAP=$(sql -c "SELECT stream_id FROM checkout_snapshot ORDER BY at DESC LIMIT 1" | tr -d ' \r\n')
[ -n "$SNAP" ] || { echo "  FAILED: snapshot_every declared and no snapshot saved"; exit 1; }
ver=$(sql -c "SELECT version FROM checkout_snapshot WHERE stream_id = '$SNAP' ORDER BY version DESC LIMIT 1" | tr -d ' \r\n')
if [ $((ver % every)) -eq 0 ]; then
  echo "  OK: snapshot at version $ver, a multiple of the declared cadence ($every)"
else
  echo "  FAILED: snapshot at version $ver and the declared cadence is $every"
  exit 1
fi

# What makes a snapshot safe: rehydrating from it gives EXACTLY the same as
# replaying the whole stream. The saved state is compared against the one in the
# view, which was built event by event without using snapshots.
echo "  the snapshot against the view, which was built without snapshots"
in_snap=$(sql -c "SELECT state->>'state' FROM checkout_snapshot WHERE stream_id = '$SNAP' ORDER BY version DESC LIMIT 1" | tr -d ' \r\n')
in_view=$(sql -c "SELECT state FROM view_conversion WHERE stream_id = '$SNAP'" | tr -d ' \r\n')
if [ "$in_snap" = "$in_view" ]; then
  echo "  OK: the snapshot says '$in_snap' and the projection, which did not use it, says the same"
else
  echo "  FAILED: snapshot '$in_snap' against view '$in_view'"
  exit 1
fi

# And the part that turns the silent failure into one that fixes itself: a
# snapshot from ANOTHER rules version is ignored. One gets poisoned on purpose
# and the service has to keep giving the right state.
echo "  a snapshot with old rules, poisoned on purpose"
sql -v ON_ERROR_STOP=1 -c "INSERT INTO checkout_snapshot (stream_id, version, rules, state)
  VALUES ('$SNAP', $ver, $((rules - 1)),
          '{\"state\":\"garbage\",\"cents\":-1,\"paymentId\":null}'::jsonb)" > /dev/null
# a new event on the SAME stream has to come out of the real state, not out of
# the poisoned snapshot
last=$(sql -c "SELECT max(version) FROM checkout_event WHERE stream_id = '$SNAP'" | tr -d ' \r\n')
sql -v ON_ERROR_STOP=1 -c "INSERT INTO checkout_event (id, stream_id, version, type, data)
  VALUES (gen_random_uuid(), '$SNAP', $((last + 1)), 'checkout.compensated@v1',
          '{\"streamId\":\"$SNAP\",\"reason\":\"snapshot test\"}'::jsonb)" > /dev/null
good=$(sql -c "SELECT count(*) FROM checkout_snapshot WHERE stream_id = '$SNAP' AND rules = $rules" | tr -d ' \r\n')
bad=$(sql -c "SELECT count(*) FROM checkout_snapshot WHERE stream_id = '$SNAP' AND rules <> $rules" | tr -d ' \r\n')
cents=$(sql -c "SELECT (state->>'cents')::bigint FROM checkout_snapshot
                 WHERE stream_id = '$SNAP' AND rules = $rules ORDER BY version DESC LIMIT 1" | tr -d ' \r\n')
if [ "$bad" -ge 1 ] && [ "$good" -ge 1 ] && [ "$cents" -gt 0 ]; then
  echo "  OK: the rules $((rules - 1)) snapshot lives alongside the current one, which says $cents cents"
  echo "  i that the code IGNORES it is what the testkit proves: here what is checked"
  echo "    is that a snapshot from another version does not overwrite the current"
  echo "    one. Bumping snapshot_version is what turns 'the snapshot went wrong'"
  echo "    into 'the snapshot gets rebuilt'"
else
  echo "  FAILED: good=$good bad=$bad cents=$cents"
  exit 1
fi

# --- rebuilding the view -------------------------------------------------
# It is what turns a read model into something whose SHAPE can be changed without
# a migration: change the projection, rebuild, and there is no ALTER TABLE
# preserving data that can be recomputed.
#
# It is measured by dirtying the view on purpose: if the rebuild did not fix the
# garbage, it would not be rebuilding anything.
echo "  the view, dirtied on purpose and rebuilt"
sql -v ON_ERROR_STOP=1 -c "UPDATE view_conversion SET state = 'garbage', cents = -1" > /dev/null
dirty=$(sql -c "SELECT count(*) FROM view_conversion WHERE state = 'garbage'" | tr -d ' \r\n')
[ "$dirty" -ge 1 ] || { echo "  FAILED: there was nothing to dirty"; exit 1; }
echo "    $dirty rows with garbage"

applied=$(curl -sS --fail-with-body -m 120 -X POST "$CHECKOUT/internal/view/conversion/rebuild" \
  | sed 's/.*"applied":\([0-9]*\).*/\1/')
in_stream=$(sql -c "SELECT count(*) FROM checkout_event" | tr -d ' \r\n')
left=$(sql -c "SELECT count(*) FROM view_conversion WHERE state = 'garbage' OR cents < 0" | tr -d ' \r\n')
if [ "$left" -eq 0 ] && [ "$applied" -ge 1 ]; then
  echo "  OK: it applied $applied events of the stream's $in_stream, and no garbage was left"
else
  echo "  FAILED: $left dirty rows left, applied=$applied"
  exit 1
fi

# And what was rebuilt matches the stream, event by event: if it differed, the
# rebuild would be giving a different answer from the live projection.
different=$(sql -c "
  WITH last_ev AS (
    SELECT stream_id, replace(split_part(type, '.', 2), '@v1', '') AS state,
           row_number() OVER (PARTITION BY stream_id ORDER BY version DESC) AS r
      FROM checkout_event
  )
  SELECT count(*) FROM last_ev u
    JOIN view_conversion v ON v.stream_id = u.stream_id
   WHERE u.r = 1 AND v.state <> u.state" | tr -d ' \r\n')
if [ "$different" -eq 0 ]; then
  echo "  OK: every rebuilt row matches the last event of its stream"
else
  echo "  FAILED: $different rows do not match the stream"
  exit 1
fi

# The dates are the STREAM's, not the rebuild's. Filling them with `now()` would
# rewrite history and the lag measured afterwards would be false.
future=$(sql -c "SELECT count(*) FROM view_conversion v
                  WHERE v.event_at > (SELECT max(at) FROM checkout_event
                                       WHERE stream_id = v.stream_id)" | tr -d ' \r\n')
if [ "$future" -eq 0 ]; then
  echo "  OK: the dates came from the stream, not from the hour of the rebuild"
else
  echo "  FAILED: $future rows with a date later than their last event"
  exit 1
fi

# --- the shadow: nobody sees a half-built view ---------------------------
# Rebuilding in place leaves the view incomplete while it runs, and it keeps
# being read: whoever asks gets fewer rows than there are, with no error at all.
# With a shadow, the live view keeps answering what it did before until the swap.
#
# It is measured by stretching the rebuild on purpose and counting rows WHILE it
# runs: without the switch it finishes in milliseconds and "nobody saw anything"
# cannot be told apart from "nobody looked".
echo "  the reads while the view is being rebuilt"
rows_before=$(sql -c "SELECT count(*) FROM view_conversion" | tr -d ' \r\n')
[ "$rows_before" -ge 3 ] || { echo "  FAILED: rows are needed to be able to measure the window"; exit 1; }
cp "$ENVQ" "$ENVQ.es"
printf 'AXON_DEMO_REBUILD_SLOW_MS=150\n' >> "$ENVQ"
$COMPOSE stop checkout > /dev/null 2>&1
$COMPOSE up -d --wait checkout > /dev/null 2>&1

curl -sS -m 180 -X POST "$CHECKOUT/internal/view/conversion/rebuild" > /tmp/axon-rebuild.json &
rebuild=$!
minimum=$rows_before
i=0
while kill -0 "$rebuild" 2>/dev/null; do
  i=$((i + 1))
  [ "$i" -gt 200 ] && break
  n=$(sql -c "SELECT count(*) FROM view_conversion" 2>/dev/null | tr -d ' \r\n')
  case "$n" in ''|*[!0-9]*) continue ;; esac
  [ "$n" -lt "$minimum" ] && minimum=$n
done
wait "$rebuild" || true
applied=$(sed 's/.*"applied":\([0-9]*\).*/\1/' /tmp/axon-rebuild.json)
rows_after=$(sql -c "SELECT count(*) FROM view_conversion" | tr -d ' \r\n')
echo "    $i reads during the rebuild; lowest seen: $minimum of $rows_before"
if [ "$i" -ge 3 ] && [ "$minimum" -ge "$rows_before" ] && [ "$rows_after" -ge "$rows_before" ]; then
  echo "  OK: nobody saw a half-built view; $applied events applied in the shadow"
  echo "  i rebuilding in place, the lowest would have been 0"
else
  echo "  FAILED: lowest=$minimum before=$rows_before after=$rows_after reads=$i"
  exit 1
fi
restore_env
$COMPOSE stop checkout > /dev/null 2>&1
$COMPOSE up -d --wait checkout > /dev/null 2>&1

# --- pruning the snapshots -----------------------------------------------
# `snapshot_version` invalidates the old snapshots but does not remove them, so
# the table grows with every rules version. The prune can be aggressive because a
# snapshot is a cache: the worst that happens is rebuilding from the stream.
echo "  the prune of the snapshots the current version does not use"
before=$(sql -c "SELECT count(*) FROM checkout_snapshot" | tr -d ' \r\n')
old=$(sql -c "SELECT count(*) FROM checkout_snapshot WHERE rules <> $rules" | tr -d ' \r\n')
[ "$old" -ge 1 ] || { echo "  FAILED: the setup left no snapshot from another version"; exit 1; }
deleted=$(curl -sS --fail-with-body -m 30 -X POST "$CHECKOUT/internal/aggregate/checkout/prune" \
  | sed 's/.*"deleted":\([0-9]*\).*/\1/')
after=$(sql -c "SELECT count(*) FROM checkout_snapshot" | tr -d ' \r\n')
old_left=$(sql -c "SELECT count(*) FROM checkout_snapshot WHERE rules <> $rules" | tr -d ' \r\n')
echo "    $before snapshots, deleted $deleted, $after left"
if [ "$old_left" -eq 0 ] && [ "$deleted" -ge "$old" ] && [ "$after" -lt "$before" ]; then
  echo "  OK: the ones from another version are gone, and only the newest of each stream is left"
else
  echo "  FAILED: $old_left from another version left; $before -> $after, deleted=$deleted"
  exit 1
fi

# One per stream at most: more than one is space nobody reads, because
# `snapshot()` always takes the newest.
duplicates=$(sql -c "SELECT coalesce(max(n), 0) FROM (
                       SELECT count(*) AS n FROM checkout_snapshot GROUP BY stream_id
                     ) t" | tr -d ' \r\n')
[ "$duplicates" -le 1 ] || { echo "  FAILED: one stream was left with $duplicates snapshots"; exit 1; }

# And what makes the prune safe: the state stays correct. The snapshot is redone
# at the next event, and in the meantime it is rebuilt from the stream.
echo "  and the state after running out of snapshots"
sql -v ON_ERROR_STOP=1 -c "DELETE FROM checkout_snapshot" > /dev/null
r=$(curl -sS --fail-with-body -m 60 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d '{"orderId":"55555555-5555-4555-8555-555555555555","amount":{"amount":700,"currency":"MXN"}}')
case "$r" in
  *completed*) echo "  OK: with no snapshot at all the system stays correct, it just rebuilds more" ;;
  *) echo "  FAILED: with no snapshots the checkout returned $r"; exit 1 ;;
esac

# --- the outbox, really transactional ------------------------------------
# The pattern's promise is that the event and the state change get in together or
# neither does. If `stage` opened its own connection, a rolled-back transaction
# would leave the event WITHOUT its row and the relay would publish something
# that never happened. Nobody sees it until somebody asks about an event with no
# payment.
#
# This was broken and it was measured before being fixed: 0 payments and 1 event.
echo "  a transaction rolled back after staging the event"
sq() { $COMPOSE exec -T db-payments env PGPASSWORD=local psql -qtAX -U postgres -d payments -c "$1"; }
cp "$ENVQ" "$ENVQ.es"
printf 'AXON_DEMO_BREAK_AFTER_STAGE=1\n' >> "$ENVQ"
$COMPOSE stop payments > /dev/null 2>&1
$COMPOSE up -d --wait payments > /dev/null 2>&1

ORDER=$(sq "SELECT gen_random_uuid()" | tr -d ' \r\n')
code=$(curl -sS -o /dev/null -w '%{http_code}' -m 30 -X POST "localhost:${AXON_PORT_payments:-8082}/v1/payments" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDER\",\"amount\":{\"amount\":500,\"currency\":\"MXN\"}}")
payments=$(sq "SELECT count(*) FROM payment WHERE order_id = '$ORDER'" | tr -d ' \r\n')
events=$(sq "SELECT count(*) FROM outbox WHERE data->>'orderId' = '$ORDER'" | tr -d ' \r\n')
restore_env
$COMPOSE stop payments > /dev/null 2>&1
$COMPOSE up -d --wait payments > /dev/null 2>&1
if [ "$code" = "500" ] && [ "$payments" -eq 0 ] && [ "$events" -eq 0 ]; then
  echo "  OK: 0 payments and 0 events; the event does not survive the rollback"
  echo "  i with \`stage\` on its own connection this gave 0 and 1: a charge"
  echo "    published that never happened, and nothing saying so"
else
  echo "  FAILED: http=$code payments=$payments events=$events"
  exit 1
fi
