#!/bin/sh
# The saga, measured against the containers: that it compensates when the second
# step fails, and that the sweep resumes one left stranded in ANOTHER process.
#
# The second part is what cannot be checked with a closure: on resume, the only
# thing left of what step 1 did is what the journal saved.
set -eu
cd "$(dirname "$0")"
COMPOSE="docker compose -f axon.local.yml"
CHECKOUT="localhost:${AXON_PORT_checkout:-8081}"
PAYMENTS="localhost:${AXON_PORT_payments:-8082}"
CEILING=100000   # the merchant's ceiling, in payments' code

# A fresh id per run: asserting over counts accumulated from earlier runs is
# asserting over something else.
uuid() { $COMPOSE exec -T db-checkout env PGPASSWORD=local psql -qtAX -U postgres -d checkout -c 'SELECT gen_random_uuid()' | tr -d ' \r\n'; }

sql() { $COMPOSE exec -T db-checkout env PGPASSWORD=local psql -qtAX -U postgres -d checkout "$@"; }
sql_payments() { $COMPOSE exec -T db-payments env PGPASSWORD=local psql -qtAX -U postgres -d payments "$@"; }

# --- the happy path -------------------------------------------------------
HAPPY=$(uuid)
echo "  a checkout below the ceiling"
r=$(curl -sS --fail-with-body -m 60 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$HAPPY\",\"amount\":{\"amount\":2500,\"currency\":\"MXN\"}}")
echo "    $r"
case "$r" in *completed*) : ;; *) echo "  FAILED: the checkout did not complete"; exit 1 ;; esac
# and the merchant WAS paid, for this order
paid=$(sql_payments -c "SELECT count(*) FROM payout p JOIN payment m ON m.id = p.payment_id WHERE m.order_id = '$HAPPY'")
[ "$paid" -eq 1 ] || { echo "  FAILED: payouts=$paid for the happy checkout"; exit 1; }

# --- the compensation ----------------------------------------------------
echo "  a checkout ABOVE the ceiling: step 2 fails after the charge"
BROKEN=$(uuid)
r=$(curl -sS --fail-with-body -m 60 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$BROKEN\",\"amount\":{\"amount\":$((CEILING + 1)),\"currency\":\"MXN\"}}")
echo "    $r"
case "$r" in *compensated*) : ;; *) echo "  FAILED: it did not compensate"; exit 1 ;; esac
# The invariant, not the count: of this order NO charge is left standing, and the
# merchant was paid nothing.
standing=$(sql_payments -c "SELECT count(*) FROM payment WHERE order_id = '$BROKEN' AND status <> 'refunded'")
refunded=$(sql_payments -c "SELECT count(*) FROM payment WHERE order_id = '$BROKEN' AND status = 'refunded'")
payouts=$(sql_payments -c "SELECT count(*) FROM payout p JOIN payment m ON m.id = p.payment_id WHERE m.order_id = '$BROKEN'")
if [ "$standing" -eq 0 ] && [ "$refunded" -ge 1 ] && [ "$payouts" -eq 0 ]; then
  echo "  OK: the charge was undone and the merchant was not paid"
else
  echo "  FAILED: standing=$standing refunded=$refunded payouts=$payouts"
  exit 1
fi
# and the journal says so
status=$(sql -c "SELECT status FROM saga_checkout ORDER BY updated DESC LIMIT 1")
[ "$status" = "compensated" ] || { echo "  FAILED: the journal says '$status'"; exit 1; }

# --- the resume, which is what the journal makes possible ----------------
# A saga that started in another process: step 1 was left `done` and the process
# died before step 2. The only thing left of step 1 is what the journal saved,
# and the paymentId to compensate with has to come from there.
echo "  a saga stranded in another process, resumed by the sweep"
ORDER=$(uuid)
# `payments:write`, que es lo que el manifiesto exige para cobrar: sin el, el
# servicio contesta 403 y hace bien.
charge=$(curl -sS --fail-with-body -m 30 -X POST "$PAYMENTS/v1/payments" \
  -H 'x-granted-scopes: payments:write' \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDER\",\"amount\":{\"amount\":$((CEILING + 1)),\"currency\":\"MXN\"}}")
payment=$(printf '%s' "$charge" | sed 's/.*"paymentId":"\([^"]*\)".*/\1/')
[ -n "$payment" ] || { echo "  FAILED: could not charge for the setup"; exit 1; }

SAGA=$(sql -c "SELECT gen_random_uuid()")
sql -v ON_ERROR_STOP=1 -c "INSERT INTO saga_checkout (id, step, status, data, outputs, updated)
  VALUES ('$SAGA', 1, 'done',
    '{\"id\":\"$SAGA\",\"type\":\"POST /v1/checkouts\",\"source\":\"demo\",\"time\":\"2026-01-01T00:00:00Z\",
      \"traceparent\":\"00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01\",
      \"correlationId\":\"$SAGA\",\"causationId\":null,
      \"data\":{\"orderId\":\"$ORDER\",\"amount\":{\"amount\":$((CEILING + 1)),\"currency\":\"MXN\"}}}'::jsonb,
    jsonb_build_object('1', jsonb_build_object('paymentId', '$payment')),
    now() - interval '10 minutes')" > /dev/null

sweep=$(curl -sS --fail-with-body -m 90 -X POST "$CHECKOUT/internal/saga/checkout/sweep")
echo "    $sweep"
case "$sweep" in
  *'"compensated":1'*) : ;;
  *) echo "  FAILED: the sweep did not compensate the stranded one"; exit 1 ;;
esac
# the refund came from the paymentId the JOURNAL saved, not from a variable
status=$(sql_payments -c "SELECT status FROM payment WHERE id = '$payment'")
[ "$status" = "refunded" ] || { echo "  FAILED: the payment was left in '$status'"; exit 1; }
final=$(sql -c "SELECT status FROM saga_checkout WHERE id = '$SAGA'")
[ "$final" = "compensated" ] || { echo "  FAILED: the journal says '$final'"; exit 1; }
echo "  OK: resumed from the journal, compensated, and the refund reached the charge"

# and it does not take it again
again=$(curl -sS -m 60 -X POST "$CHECKOUT/internal/saga/checkout/sweep")
case "$again" in
  *'"claimed":0'*) echo "  OK: a closed saga is not swept again" ;;
  *) echo "  FAILED: the sweep claimed an already closed saga: $again"; exit 1 ;;
esac
