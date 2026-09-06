#!/bin/sh
# Los reintentos declarados, medidos.
#
# Lo declarado sale del CODIGO GENERADO —`withPolicy(..., { retries: N })`—
# y no de un numero escrito aqui: comparar contra una copia a mano no compara
# nada. Lo medido sale de la tabla `intento` de payments, que registra cada
# llamada que llego.
set -eu
cd "$(dirname "$0")"
COMPOSE="docker compose -f axon.local.yml"
CHECKOUT="localhost:${AXON_PORT_checkout:-8081}"
PAGOS="localhost:${AXON_PORT_payments:-8082}"
TOPE=100000

sql_pagos() { $COMPOSE exec -T db-payments env PGPASSWORD=local psql -qtAX -U postgres -d payments "$@"; }

# Los interruptores viajan por `.env.local`, que es el `env_file` que el compose
# generado ya monta: una variable en el shell no entra al contenedor si el
# compose no la declara, y declararla ahi seria meter algo del demo en la
# infraestructura generada.
ENV=.env.local
cp "$ENV" "$ENV.demo"
restaurar() { mv -f "$ENV.demo" "$ENV" 2>/dev/null || true; }
trap 'restaurar' EXIT

payments_con() {
  restaurar
  cp "$ENV" "$ENV.demo"
  for kv in "$@"; do printf '%s\n' "$kv" >> "$ENV"; done
  $COMPOSE stop payments >/dev/null 2>&1
  $COMPOSE up -d --wait payments >/dev/null 2>&1
}
uuid() { sql_pagos -c 'SELECT gen_random_uuid()' | tr -d ' \r\n'; }

# lo declarado, leido del generado
declarado() {
  sed -n "s/.*withPolicy(\"payments\.$1\", { timeoutMs: \([0-9]*\), retries: \([0-9]*\).*/\2/p" \
    services/checkout/contracts.ts | head -1
}
# el presupuesto de la SAGA, no el del metodo: los dos se llaman `timeout_ms`
# en el manifiesto, y el generado deja solo uno de los dos en el coordinador
presupuesto=$(sed -n 's/.*const limite = Date.now() + \([0-9]*\);.*/\1/p' \
  services/checkout/contracts.ts | head -1)

r_payout=$(declarado payoutMerchant)
r_refund=$(declarado refundPayment)
echo "  declarado en el generado: payout $r_payout reintentos, refund $r_refund"

# --- que los reintentos OCURRAN --------------------------------------------
# El payout tarda mas que su propio timeout, asi que cada intento se agota y el
# cliente reintenta segun la politica. Son $r_payout + 1 llamadas, ni una mas.
echo "  payout mas lento que su timeout: cuantas veces llega"
payments_con AXON_DEMO_PAYOUT_LENTO_MS=6000
ORDEN=$(uuid)
inicio=$(date +%s)
r=$(curl -sS --fail-with-body -m 120 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDEN\",\"amount\":{\"amount\":2500,\"currency\":\"MXN\"}}")
fin=$(date +%s)
echo "    $r  ($((fin - inicio))s)"
pago=$(sql_pagos -c "SELECT id FROM payment WHERE order_id = '$ORDEN' LIMIT 1" | tr -d ' \r\n')
intentos=$(sql_pagos -c "SELECT count(*) FROM intento WHERE metodo = 'payout' AND payment_id = '$pago'")
if [ "$intentos" -eq $((r_payout + 1)) ]; then
  echo "  OK: $intentos llamadas = 1 + $r_payout reintentos, exactamente lo declarado"
else
  echo "  FALLO: $intentos llamadas, se declararon $((r_payout + 1))"
  exit 1
fi
# y el flujo entero cabe en el presupuesto declarado: es lo que hace que
# rendirse por tiempo signifique algo
transcurrido=$(( (fin - inicio) * 1000 ))
if [ "$transcurrido" -le "$presupuesto" ]; then
  echo "  OK: ${transcurrido}ms dentro del presupuesto de ${presupuesto}ms"
else
  echo "  FALLO: ${transcurrido}ms supera el presupuesto de ${presupuesto}ms"
  exit 1
fi
case "$r" in *compensada*) : ;; *) echo "  FALLO: agotar los reintentos no compenso"; exit 1 ;; esac

# --- que los reintentos de la COMPENSACION sean lo que la salva ------------
# El reembolso falla las dos primeras veces. Con $r_refund reintentos
# declarados hay margen, asi que la saga tiene que terminar `compensada` y no
# `atascada`: eso es exactamente lo que compran esos reintentos.
echo "  reembolso que falla 2 veces antes de entrar"
payments_con AXON_DEMO_REFUND_FALLAR_VECES=2
ORDEN=$(uuid)
r=$(curl -sS --fail-with-body -m 120 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDEN\",\"amount\":{\"amount\":$((TOPE + 1)),\"currency\":\"MXN\"}}")
echo "    $r"
pago=$(sql_pagos -c "SELECT id FROM payment WHERE order_id = '$ORDEN' LIMIT 1" | tr -d ' \r\n')
reintentos=$(sql_pagos -c "SELECT count(*) FROM intento WHERE metodo = 'refund' AND payment_id = '$pago'")
estado=$(sql_pagos -c "SELECT status FROM payment WHERE id = '$pago'")
if [ "$r" != "${r%compensada*}" ] && [ "$reintentos" -eq 3 ] && [ "$estado" = "refunded" ]; then
  echo "  OK: 3 llamadas al reembolso (2 fallos y la que entro), y el cobro quedo deshecho"
  echo "  i sin los $r_refund reintentos declarados, esta saga terminaba ATASCADA"
else
  echo "  FALLO: llamadas=$reintentos estado=$estado respuesta=$r"
  exit 1
fi

# --- que agotarlos deje la saga atascada, y se note -----------------------
# Mas fallos que reintentos: la compensacion no entra, y eso NO puede quedar en
# silencio. Tiene que verse en el diario y en la respuesta.
echo "  reembolso que falla mas veces que los reintentos declarados"
payments_con "AXON_DEMO_REFUND_FALLAR_VECES=$((r_refund + 2))"
ORDEN=$(uuid)
codigo=0
r=$(curl -sS -m 120 -o /dev/null -w '%{http_code}' -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDEN\",\"amount\":{\"amount\":$((TOPE + 1)),\"currency\":\"MXN\"}}") || codigo=$?
echo "    HTTP $r"
pago=$(sql_pagos -c "SELECT id FROM payment WHERE order_id = '$ORDEN' LIMIT 1" | tr -d ' \r\n')
llamadas=$(sql_pagos -c "SELECT count(*) FROM intento WHERE metodo = 'refund' AND payment_id = '$pago'")
atascada=$($COMPOSE exec -T db-checkout env PGPASSWORD=local psql -qtAX -U postgres -d checkout \
  -c "SELECT count(*) FROM saga_compra WHERE estado = 'atascada'")
if [ "$r" = "500" ] && [ "$llamadas" -eq $((r_refund + 1)) ] && [ "$atascada" -ge 1 ]; then
  echo "  OK: $llamadas llamadas, la saga quedo ATASCADA y la respuesta no lo oculto"
else
  echo "  FALLO: http=$r llamadas=$llamadas atascadas=$atascada"
  exit 1
fi

# dejar payments como estaba: los interruptores son del demo, no del servicio
restaurar
$COMPOSE stop payments >/dev/null 2>&1
$COMPOSE up -d --wait payments >/dev/null 2>&1
echo "  payments restaurado sin interruptores"
