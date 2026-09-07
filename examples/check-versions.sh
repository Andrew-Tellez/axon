#!/bin/sh
# Dos versiones del mismo endpoint, y el retiro de la vieja, medidos.
#
# Lo declarado sale del CODIGO GENERADO —`retiredRoutes`— y no de una fecha
# escrita aca. Lo medido son los headers que realmente salen del contenedor.
set -eu
cd "$(dirname "$0")"
ORDERS="localhost:${AXON_PORT_orders:-8080}"
TENANT="${AXON_TENANT:-11111111-1111-4111-8111-111111111111}"

# What is declared comes from the line the router really serves —the one in
# `retiredRoutes`— and not from the manifest embedded in the same file: that one
# carries the ISO date of the manifest, and the header is an HTTP-date. Reading
# the wrong one is comparing two formats and calling it a difference.
retired=$(grep '"deprecation": "@' services/orders/contracts.ts | head -1)
declared_dep=$(printf '%s' "$retired" | sed -n 's/.*"deprecation": "\(@[0-9]*\)".*/\1/p')
declared_sunset=$(printf '%s' "$retired" | sed -n 's/.*"sunset": "\([^"]*\)".*/\1/p')
echo "  declared in the generated code: deprecation $declared_dep, sunset $declared_sunset"

order=$(curl -sS --fail-with-body -m 30 -X POST "$ORDERS/v1/tenants/$TENANT/orders" \
  -H 'content-type: application/json' \
  -d '{"customerId":"11111111-1111-4111-8111-111111111111","total":{"amount":4200,"currency":"MXN"}}' \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["orderId"])')

# --- the retired version keeps answering, and says so ---------------------
# Deprecated is not gone: whoever calls it today has to keep working, and find
# out on the way out. Both things, or the announcement is a 404 with no notice.
echo "  the v1 of the endpoint: what it answers and what it announces"
headers=$(curl -sS -D- -o /tmp/axon-v1.json -m 30 "$ORDERS/v1/tenants/$TENANT/orders/$order")
code=$(printf '%s' "$headers" | sed -n 's/^HTTP\/1.1 \([0-9]*\).*/\1/p' | tr -d '\r')
dep=$(printf '%s' "$headers" | sed -n 's/^[Dd]eprecation: //p' | tr -d '\r')
sunset=$(printf '%s' "$headers" | sed -n 's/^[Ss]unset: //p' | tr -d '\r')
link=$(printf '%s' "$headers" | sed -n 's/^[Ll]ink: //p' | tr -d '\r')
echo "    HTTP $code  Deprecation: $dep  Sunset: $sunset"
echo "    Link: $link"
if [ "$code" = "200" ] && [ "$dep" = "$declared_dep" ] && [ "$sunset" = "$declared_sunset" ]; then
  echo "  OK: it still answers 200 and goes out with the declared Deprecation and Sunset"
else
  echo "  FAILED: code=$code deprecation=$dep sunset=$sunset"
  exit 1
fi
case "$link" in
  *'rel="successor-version"'*) : ;;
  *) echo "  FAILED: the Link carries no successor-version: $link"; exit 1 ;;
esac
case "$link" in
  *'</v2/'*) echo "  OK: the Link points at the v2, so nobody has to guess where to go" ;;
  *) echo "  FAILED: the Link does not point at the successor: $link"; exit 1 ;;
esac

# --- and the v2 is the same endpoint, not another service -----------------
# Same row, one more field, and no announcement of its own: the current version
# has nothing to announce.
echo "  the v2 of the same endpoint"
v2=$(curl -sS -D/tmp/axon-v2.head -m 30 "$ORDERS/v2/tenants/$TENANT/orders/$order")
echo "    $v2"
printf '%s' "$v2" > /tmp/axon-v2.txt
same=$(python3 -c '
import json
a = json.load(open("/tmp/axon-v1.json")); b = json.load(open("/tmp/axon-v2.txt"))
print("yes" if all(a[k] == b[k] for k in a) and "customerId" in b else "no")')
announces=$(sed -n 's/^[Dd]eprecation: //p' /tmp/axon-v2.head | tr -d '\r')
if [ "$same" = "yes" ] && [ -z "$announces" ]; then
  echo "  OK: the v2 answers the same plus the customer, and announces nothing: it is the current one"
else
  echo "  FAILED: same=$same deprecation='$announces'"
  exit 1
fi
echo "  i and axon verify names who still calls the v1, which inside one repo is a grep"
