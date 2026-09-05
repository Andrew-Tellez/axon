#!/bin/sh
# Demo end-to-end: levanta el sistema, dispara un flujo y comprueba que la
# cadena causal real coincide con la que declara el manifiesto.
#
#   cd examples && ./demo.sh
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"

# Los puertos por defecto los deriva axon del NOMBRE del servicio, para que
# agregar uno no le mueva el puerto a otro. El demo los fija a proposito: asi el
# script y el compose hablan del mismo numero sin leerse entre ellos.
export AXON_PORT_orders="${AXON_PORT_orders:-8080}"
export AXON_PORT_checkout="${AXON_PORT_checkout:-8081}"
export AXON_PORT_payments="${AXON_PORT_payments:-8082}"
PORT="$AXON_PORT_orders"

# Cuando esto falla en CI no hay nadie mirando la pantalla: el diagnostico
# tiene que quedar en el log del run, o se pierde.
PASO="inicio"
paso() {
  PASO="$1"
  echo "==> $1"
}

diagnostico() {
  codigo=$?
  [ "$codigo" -eq 0 ] && return 0
  echo
  echo "==> FALLO en el paso '$PASO' (codigo $codigo)"
  # Actions corta a 10 anotaciones por paso, asi que lo esencial va primero y
  # en una sola: sin esto el resumen del fallo se pierde entre los logs.
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    estado=$(docker compose -f axon.local.yml ps -a --format '{{.Service}}={{.State}}' 2>/dev/null | tr '\n' ' ')
    echo "::error title=fallo::paso='$PASO' codigo=$codigo contenedores: $estado"
  fi
  docker compose -f axon.local.yml ps -a || true
  for s in $(docker compose -f axon.local.yml config --services 2>/dev/null); do
    echo
    echo "--- logs de $s (ultimas 40) ---"
    docker compose -f axon.local.yml logs --tail 40 "$s" 2>&1 || true
    # En Actions el log del run no es publico, pero las anotaciones si: las
    # ultimas lineas de cada servicio salen tambien como ::error::
    # solo de los que no terminaron bien: el resto llenaria el cupo de anotaciones
    if [ -n "${GITHUB_ACTIONS:-}" ]; then
      case "$(docker compose -f axon.local.yml ps -a --format '{{.State}}' --status running --status exited "$s" 2>/dev/null)" in
        running|exited) : ;;
        *) docker compose -f axon.local.yml logs --tail 8 --no-log-prefix "$s" 2>&1 \
             | tr '\n' '|' | sed "s/^/::error title=$s::/" || true ;;
      esac
    fi
  done
  echo
  echo "--- envelopes registrados ---"
  cat .axon/local.ndjson 2>/dev/null || echo "(ninguno)"
  return "$codigo"
}
trap diagnostico EXIT

paso "generando la infraestructura local desde los manifiestos"
"$AXON" infra . --target local > axon.local.yml

mkdir -p .axon
# flagd lee este JSON: los flags tambien salen del manifiesto
"$AXON" flags . > .axon/flags.json

# pgdog lee dos archivos, y los dos salen del manifiesto. Los hosts que nombra
# `--target local` son los contenedores que acaba de emitir `axon infra`.
mkdir -p .axon/pgdog/orders
"$AXON" pooler . --service orders --target local > .axon/pgdog/orders/pgdog.toml
"$AXON" pooler . --service orders --target local --users > .axon/pgdog/orders/users.toml

paso "levantando broker, bases, migraciones y servicios"
docker compose -f axon.local.yml up -d --build --wait

rm -f .axon/local.ndjson
mkdir -p .axon

TENANT="${AXON_TENANT:-11111111-1111-4111-8111-111111111111}"
paso "POST /v1/tenants/{tenantId}/orders"
if ! respuesta=$(curl -sS --fail-with-body --max-time 30 \
    -X POST "localhost:$PORT/v1/tenants/$TENANT/orders" \
    -H 'content-type: application/json' \
    -d '{"customerId":"11111111-1111-4111-8111-111111111111","total":{"amount":25000,"currency":"MXN"}}' 2>&1); then
  codigo=$?
  echo "curl salio $codigo: $respuesta"
  [ -n "${GITHUB_ACTIONS:-}" ] && echo "::error title=POST::curl=$codigo respuesta=$respuesta"
  exit 1
fi
echo "$respuesta"

paso "esperando a que la cadena se propague"
i=0
while [ "$(wc -l < .axon/local.ndjson 2>/dev/null || echo 0)" -lt 3 ]; do
  i=$((i + 1))
  [ "$i" -gt 45 ] && { echo "la cadena no se completo"; exit 1; }
  sleep 1
done

echo
paso "cadena causal real"
"$AXON" trace .axon/local.ndjson
echo
paso "la traza en OpenTelemetry"
UI="localhost:${AXON_TRAZA_UI_PORT:-16686}"
i=0
until curl -fsS "http://$UI/api/services" 2>/dev/null | grep -q payments; do
  i=$((i + 1))
  [ "$i" -gt 45 ] && { echo "no llego ninguna traza al colector"; exit 1; }
  sleep 1
done
python3 verificar-traza.py "$UI"

paso "esperado (manifiesto) vs real (log de envelopes)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# Se acota al flujo que disparo este demo. Comparar un flujo esperado contra
# TODOS los del log solo funciona si hay exactamente uno, y eso deja de ser
# cierto en cuanto algo mas toca el sistema —una prueba de carga, por ejemplo.
FLUJO=$(python3 -c 'import json;print(json.loads(open(".axon/local.ndjson").readline())["correlationId"])')
"$AXON" seq order.placed@v1 . --events > "$tmp/esperado"
"$AXON" trace .axon/local.ndjson --seq --correlation "$FLUJO" > "$tmp/real"
if diff -u "$tmp/esperado" "$tmp/real"; then
  echo "OK: el sistema hace exactamente lo que declara"
else
  echo "DRIFT: el sistema no hace lo que declara"
  exit 1
fi


paso "aislamiento por inquilino a traves del pooler"
./verificar-pooler.sh

paso "la saga: compensacion y retome, medidos"
./verificar-saga.sh

paso "rollout declarado vs aplicado"
python3 verificar-flags.py "localhost:${AXON_FLAGS_PORT:-8016}" cobro_v2 10

paso "capacidad declarada vs medida"
if command -v k6 >/dev/null 2>&1; then
  "$AXON" load orders.toml > .axon/carga.js
  k6 run --quiet --summary-export=.axon/carga.json \
    --env AXON_BASE="http://localhost:$PORT" \
    --env AXON_CARGA_DURACION="${AXON_CARGA_DURACION:-10s}" \
    .axon/carga.js > /dev/null 2>&1 || true
  "$AXON" load orders.toml --check .axon/carga.json
else
  echo "  salteado: k6 no esta instalado"
fi

