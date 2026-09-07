#!/bin/sh
# The declared failures, measured.
#
# What is declared comes from the MANIFEST via the generated client —the
# `retriable` list `withPolicy` receives— and not from a number written here.
# What is measured comes from payments' `attempt` table, which records every
# call that arrived: that is what turns `retriable = false` from a comment into
# behaviour.
set -eu
cd "$(dirname "$0")"
COMPOSE="docker compose -f axon.local.yml"
ORDERS="localhost:${AXON_PORT_orders:-8080}"
CHECKOUT="localhost:${AXON_PORT_checkout:-8081}"
TENANT="${AXON_TENANT:-11111111-1111-4111-8111-111111111111}"
CEILING=100000

sql_payments() { $COMPOSE exec -T db-payments env PGPASSWORD=local psql -qtAX -U postgres -d payments "$@"; }
uuid() { sql_payments -c 'SELECT gen_random_uuid()' | tr -d ' \r\n'; }

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

# declared: the retries of the step, and which of its failures the generated
# client considers worth another try
retries=$(sed -n 's/.*withPolicy("payments\.payoutMerchant", { timeoutMs: [0-9]*, retries: \([0-9]*\).*/\1/p' \
  services/checkout/contracts.ts | head -1)
retriable=$(sed -n 's/.*(code) => \[\(.*\)\].includes(code));/\1/p' services/checkout/contracts.ts | tail -1)
echo "  declared in the generated code: payout $retries retries, retriable $retriable"
case "$retriable" in *rail_busy*) : ;; *) echo "  FAILED: the client does not carry the retriable codes"; exit 1 ;; esac
case "$retriable" in *merchant_ceiling*) echo "  FAILED: a final failure came out as retriable"; exit 1 ;; *) : ;; esac

# --- a FINAL failure arrives exactly once ---------------------------------
# Over the ceiling the payout fails with `merchant_ceiling`, declared
# `retriable = false`. The amount will not fit next time either, so the client
# does not try again: one call, not 1 + $retries.
echo "  a payout over the ceiling: a final failure, how many times it arrives"
ORDER=$(uuid)
r=$(curl -sS -m 120 -X POST "$CHECKOUT/v1/checkouts" -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDER\",\"amount\":{\"amount\":$((CEILING + 1)),\"currency\":\"MXN\"}}")
echo "    $r"
payment=$(sql_payments -c "SELECT id FROM payment WHERE order_id = '$ORDER' LIMIT 1" | tr -d ' \r\n')
final=$(sql_payments -c "SELECT count(*) FROM attempt WHERE method = 'payout' AND payment_id = '$payment'")
if [ "$final" -eq 1 ]; then
  echo "  OK: 1 call. The declared $retries retries were NOT spent on a failure that cannot end differently"
else
  echo "  FAILED: $final calls for a failure declared as final"
  exit 1
fi

# --- a RETRIABLE failure uses the whole budget ----------------------------
# Same route, same policy, same status family: the only thing that changes is
# `retriable = true` in the manifest. If that made no difference, this count
# would be 1 as well.
echo "  a saturated rail: a retriable failure, how many times it arrives"
payments_with AXON_DEMO_RAIL_BUSY=1
ORDER=$(uuid)
r=$(curl -sS -m 120 -X POST "$CHECKOUT/v1/checkouts" -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDER\",\"amount\":{\"amount\":2500,\"currency\":\"MXN\"}}")
echo "    $r"
payment=$(sql_payments -c "SELECT id FROM payment WHERE order_id = '$ORDER' LIMIT 1" | tr -d ' \r\n')
transient=$(sql_payments -c "SELECT count(*) FROM attempt WHERE method = 'payout' AND payment_id = '$payment'")
if [ "$transient" -eq $((retries + 1)) ]; then
  echo "  OK: $transient calls = 1 + $retries retries. Same policy, and the declaration is the only difference"
  echo "  i final $final call vs retriable $transient: that is what the declared errors buy, and it is not documentation"
else
  echo "  FAILED: $transient calls, $((retries + 1)) were declared"
  exit 1
fi
restore

# --- the code travels, and it is the declared one -------------------------
# The caller does not parse prose: it reads `title`, which is the manifest's
# code, and the status is the declared one too.
echo "  the code on the wire against the manifest"
$COMPOSE stop payments >/dev/null 2>&1; $COMPOSE up -d --wait payments >/dev/null 2>&1
body=$(curl -sS -m 30 -o /dev/null -w '%{http_code}' -X POST "$ORDERS/v1/tenants/$TENANT/orders" \
  -H 'content-type: application/json' \
  -d '{"customerId":"11111111-1111-4111-8111-111111111111","total":{"amount":0,"currency":"MXN"}}')
problem=$(curl -sS -m 30 -X POST "$ORDERS/v1/tenants/$TENANT/orders" -H 'content-type: application/json' \
  -d '{"customerId":"11111111-1111-4111-8111-111111111111","total":{"amount":0,"currency":"MXN"}}')
echo "    HTTP $body  $problem"
status=$(python3 -c 'import json,sys;print(json.loads(sys.argv[1])["status"])' "$problem")
code=$(python3 -c 'import json,sys;print(json.loads(sys.argv[1])["title"])' "$problem")
declared_status=$(python3 -c '
import re,sys
m = re.search(r"placeOrder: \[\s*\{ code: \"order_rejected\", status: (\d+)", open("services/orders/contracts.ts").read())
print(m.group(1))')
if [ "$body" = "$declared_status" ] && [ "$status" = "$declared_status" ] && [ "$code" = "order_rejected" ]; then
  echo "  OK: $code with $status, the status and the code the manifest declares"
else
  echo "  FAILED: http=$body status=$status code=$code declared=$declared_status"
  exit 1
fi
