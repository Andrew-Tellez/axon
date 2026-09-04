#!/bin/sh
# Demo end-to-end: levanta el sistema, dispara un flujo y comprueba que la
# cadena causal real coincide con la que declara el manifiesto.
#
#   cd examples && ./demo.sh
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
PORT="${AXON_PORT_orders:-8080}"

echo "==> generando la infraestructura local desde los manifiestos"
"$AXON" infra . --target local > axon.local.yml

echo "==> levantando broker, bases, migraciones y servicios"
docker compose -f axon.local.yml up -d --build --wait

rm -f .axon/local.ndjson
mkdir -p .axon

echo "==> POST /v1/orders"
curl -fsS -X POST "localhost:$PORT/v1/orders" \
  -H 'content-type: application/json' \
  -d '{"customerId":"11111111-1111-4111-8111-111111111111","total":{"amount":25000,"currency":"MXN"}}'
echo

echo "==> esperando a que la cadena se propague"
i=0
while [ "$(wc -l < .axon/local.ndjson 2>/dev/null || echo 0)" -lt 3 ]; do
  i=$((i + 1))
  [ "$i" -gt 20 ] && { echo "la cadena no se completo"; exit 1; }
  sleep 1
done

echo
echo "==> cadena causal real"
"$AXON" trace .axon/local.ndjson
echo

echo "==> esperado (manifiesto) vs real (log de envelopes)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
"$AXON" seq order.placed@v1 . --events > "$tmp/esperado"
"$AXON" trace .axon/local.ndjson --seq > "$tmp/real"
if diff -u "$tmp/esperado" "$tmp/real"; then
  echo "OK: el sistema hace exactamente lo que declara"
else
  echo "DRIFT: el sistema no hace lo que declara"
  exit 1
fi
