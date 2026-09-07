#!/bin/sh
# The declared retries, measured.
#
# What is declared comes from the GENERATED CODE —`withPolicy(..., { retries: N })`—
# and not from a number written here: comparing against a copy made by hand
# compares nothing. What is measured comes from payments' `attempt` table, which
# records every call that arrived.
set -eu
cd "$(dirname "$0")"
COMPOSE="docker compose -f axon.local.yml"
CHECKOUT="localhost:${AXON_PORT_checkout:-8081}"
PAYMENTS="localhost:${AXON_PORT_payments:-8082}"
CEILING=100000

sql_payments() { $COMPOSE exec -T db-payments env PGPASSWORD=local psql -qtAX -U postgres -d payments "$@"; }

# The switches travel through `.env.local`, which is the `env_file` the generated
# compose already mounts: a variable in the shell does not reach the container if
# the compose does not declare it, and declaring it there would be putting
# something of the demo's into the generated infrastructure.
ENV=.env.local
cp "$ENV" "$ENV.demo"
restore() { mv -f "$ENV.demo" "$ENV" 2>/dev/null || true; }
trap 'restore' EXIT

payments_with() {
  restore
  cp "$ENV" "$ENV.demo"
  for kv in "$@"; do printf '%s\n' "$kv" >> "$ENV"; done
  $COMPOSE stop payments >/dev/null 2>&1
  $COMPOSE up -d --wait payments >/dev/null 2>&1
}
uuid() { sql_payments -c 'SELECT gen_random_uuid()' | tr -d ' \r\n'; }

# what is declared, read from the generated code
declared() {
  sed -n "s/.*withPolicy(\"payments\.$1\", { timeoutMs: \([0-9]*\), retries: \([0-9]*\).*/\2/p" \
    services/checkout/contracts.ts | head -1
}
# the SAGA's budget, not the method's: both are called `timeout_ms` in the
# manifest, and the generated code leaves only one of the two in the coordinator
budget=$(sed -n 's/.*const deadline = Date.now() + \([0-9]*\);.*/\1/p' \
  services/checkout/contracts.ts | head -1)

r_payout=$(declared payoutMerchant)
r_refund=$(declared refundPayment)
echo "  declared in the generated code: payout $r_payout retries, refund $r_refund"

# --- that the retries HAPPEN ----------------------------------------------
# The payout takes longer than its own timeout, so every attempt times out and
# the client retries according to the policy. That is $r_payout + 1 calls, not
# one more.
echo "  a payout slower than its timeout: how many times it arrives"
payments_with AXON_DEMO_PAYOUT_SLOW_MS=6000
ORDER=$(uuid)
start=$(date +%s)
r=$(curl -sS --fail-with-body -m 120 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDER\",\"amount\":{\"amount\":2500,\"currency\":\"MXN\"}}")
end=$(date +%s)
echo "    $r  ($((end - start))s)"
payment=$(sql_payments -c "SELECT id FROM payment WHERE order_id = '$ORDER' LIMIT 1" | tr -d ' \r\n')
attempts=$(sql_payments -c "SELECT count(*) FROM attempt WHERE method = 'payout' AND payment_id = '$payment'")
if [ "$attempts" -eq $((r_payout + 1)) ]; then
  echo "  OK: $attempts calls = 1 + $r_payout retries, exactly what was declared"
else
  echo "  FAILED: $attempts calls, $((r_payout + 1)) were declared"
  exit 1
fi
# and the whole flow fits inside the declared budget: that is what makes giving
# up on time mean something
elapsed=$(( (end - start) * 1000 ))
if [ "$elapsed" -le "$budget" ]; then
  echo "  OK: ${elapsed}ms inside the ${budget}ms budget"
else
  echo "  FAILED: ${elapsed}ms is over the ${budget}ms budget"
  exit 1
fi
case "$r" in *compensated*) : ;; *) echo "  FAILED: exhausting the retries did not compensate"; exit 1 ;; esac

# --- that the COMPENSATION's retries are what saves it --------------------
# The refund fails the first two times. With $r_refund declared retries there is
# margin, so the saga has to end up `compensated` and not `stuck`: that is
# exactly what those retries buy.
echo "  a refund that fails twice before getting through"
payments_with AXON_DEMO_REFUND_FAIL_TIMES=2
ORDER=$(uuid)
r=$(curl -sS --fail-with-body -m 120 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDER\",\"amount\":{\"amount\":$((CEILING + 1)),\"currency\":\"MXN\"}}")
echo "    $r"
payment=$(sql_payments -c "SELECT id FROM payment WHERE order_id = '$ORDER' LIMIT 1" | tr -d ' \r\n')
retries=$(sql_payments -c "SELECT count(*) FROM attempt WHERE method = 'refund' AND payment_id = '$payment'")
status=$(sql_payments -c "SELECT status FROM payment WHERE id = '$payment'")
if [ "$r" != "${r%compensated*}" ] && [ "$retries" -eq 3 ] && [ "$status" = "refunded" ]; then
  echo "  OK: 3 calls to the refund (2 failures and the one that got through), and the charge was undone"
  echo "  i without the $r_refund declared retries, this saga ended up STUCK"
else
  echo "  FAILED: calls=$retries status=$status response=$r"
  exit 1
fi

# --- that exhausting them leaves the saga stuck, and that it shows --------
# More failures than retries: the compensation does not get through, and that
# CANNOT stay silent. It has to be visible in the journal and in the response.
echo "  a refund that fails more times than the declared retries"
payments_with "AXON_DEMO_REFUND_FAIL_TIMES=$((r_refund + 2))"
ORDER=$(uuid)
code=0
r=$(curl -sS -m 120 -o /dev/null -w '%{http_code}' -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDER\",\"amount\":{\"amount\":$((CEILING + 1)),\"currency\":\"MXN\"}}") || code=$?
echo "    HTTP $r"
payment=$(sql_payments -c "SELECT id FROM payment WHERE order_id = '$ORDER' LIMIT 1" | tr -d ' \r\n')
calls=$(sql_payments -c "SELECT count(*) FROM attempt WHERE method = 'refund' AND payment_id = '$payment'")
stuck=$($COMPOSE exec -T db-checkout env PGPASSWORD=local psql -qtAX -U postgres -d checkout \
  -c "SELECT count(*) FROM saga_checkout WHERE status = 'stuck'")
if [ "$r" = "500" ] && [ "$calls" -eq $((r_refund + 1)) ] && [ "$stuck" -ge 1 ]; then
  echo "  OK: $calls calls, the saga was left STUCK and the response did not hide it"
else
  echo "  FAILED: http=$r calls=$calls stuck=$stuck"
  exit 1
fi

# leave payments as it was: the switches belong to the demo, not to the service
restore
$COMPOSE stop payments >/dev/null 2>&1
$COMPOSE up -d --wait payments >/dev/null 2>&1
echo "  payments restored with no switches"
