#!/bin/sh
# Quien llama a que, medido en el edge.
#
# Es la mitad de la pregunta que SI tiene respuesta cuando el que llama no
# declara nada: lo que pide es observable, lo que lee de la respuesta no. Y es
# la mitad que duele al retirar una version.
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
E="localhost:${AXON_EDGE_PORT:-8000}"
T="${AXON_TENANT:-11111111-1111-4111-8111-111111111111}"

# Un cliente que no sabe nada de axon: curl contra la version vieja.
order=$(curl -sS --fail-with-body -m 30 -X POST "$E/v1/tenants/$T/orders" \
  -H 'content-type: application/json' \
  -d '{"customerId":"11111111-1111-4111-8111-111111111111","total":{"amount":1500,"currency":"MXN"}}' \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["orderId"])')
echo "  un cliente ajeno llamando cinco veces a la version deprecada"
for _ in 1 2 3 4 5; do curl -sS -o /dev/null -m 30 "$E/v1/tenants/$T/orders/$order"; done
curl -sS -o /dev/null -m 30 "$E/v2/tenants/$T/orders/$order"
sleep 1

out=$("$AXON" traffic . --check .axon/log/edge.ndjson)
echo "$out" | head -6 | sed 's/^/    /'

# lo declarado sale del manifiesto, no de un numero escrito aca
sunset=$(grep -A3 '^sunset' orders.toml | head -1 | sed 's/.*= "//; s/"//')
case "$out" in
  *"GET /v1/tenants/{tenantId}/orders/{orderId} · deprecated"*)
    echo "  OK: el log del edge nombra a quien todavia llama la version que se retira el $sunset" ;;
  *) echo "  FALLO: no reporto la ruta deprecada con trafico"; exit 1 ;;
esac
case "$out" in
  *"no traffic through the edge"*"a call between services does not pass through the edge"*)
    echo "  OK: y no afirma que nadie llama lo que el edge no puede ver" ;;
  *) echo "  FALLO: afirmo de mas sobre las rutas sin trafico"; exit 1 ;;
esac

# Y una ruta que nadie declara: un cliente apuntando a algo que no existe, o
# alguien sirviendo fuera del manifiesto. Las dos cosas hay que verlas.
curl -sS -o /dev/null -m 30 "$E/v1/inventada" || true
sleep 1
out=$("$AXON" traffic . --check .axon/log/edge.ndjson)
case "$out" in
  *undeclared*"/v1/inventada"*)
    echo "  OK: una ruta que nadie declara sale nombrada, con su conteo" ;;
  *) echo "  FALLO: no vio la ruta no declarada"; exit 1 ;;
esac
