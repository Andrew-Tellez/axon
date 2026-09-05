#!/bin/sh
# La saga, medida contra los contenedores: que compense cuando el segundo paso
# falla, y que el barrido retome una que quedo colgada en OTRO proceso.
#
# Lo segundo es lo que no se puede comprobar con una closure: al retomar, lo
# unico que queda de lo que hizo el paso 1 es lo que el diario guardo.
set -eu
cd "$(dirname "$0")"
COMPOSE="docker compose -f axon.local.yml"
CHECKOUT="localhost:${AXON_PORT_checkout:-8081}"
PAGOS="localhost:${AXON_PORT_payments:-8082}"
TOPE=100000   # el tope del comercio, en el codigo de payments

# Un id nuevo por corrida: afirmar sobre conteos acumulados de corridas
# anteriores es afirmar sobre otra cosa.
uuid() { $COMPOSE exec -T db-checkout env PGPASSWORD=local psql -qtAX -U postgres -d checkout -c 'SELECT gen_random_uuid()' | tr -d ' \r\n'; }

sql() { $COMPOSE exec -T db-checkout env PGPASSWORD=local psql -qtAX -U postgres -d checkout "$@"; }
sql_pagos() { $COMPOSE exec -T db-payments env PGPASSWORD=local psql -qtAX -U postgres -d payments "$@"; }

# --- el camino feliz ------------------------------------------------------
FELIZ=$(uuid)
echo "  una compra por debajo del tope"
r=$(curl -sS --fail-with-body -m 60 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$FELIZ\",\"amount\":{\"amount\":2500,\"currency\":\"MXN\"}}")
echo "    $r"
case "$r" in *completada*) : ;; *) echo "  FALLO: la compra no se completo"; exit 1 ;; esac
# y al comercio SI se le pago, para este pedido
pagado=$(sql_pagos -c "SELECT count(*) FROM payout p JOIN payment m ON m.id = p.payment_id WHERE m.order_id = '$FELIZ'")
[ "$pagado" -eq 1 ] || { echo "  FALLO: payouts=$pagado para la compra feliz"; exit 1; }

# --- la compensacion -----------------------------------------------------
echo "  una compra por ENCIMA del tope: el paso 2 falla despues de cobrar"
ROTA=$(uuid)
r=$(curl -sS --fail-with-body -m 60 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ROTA\",\"amount\":{\"amount\":$((TOPE + 1)),\"currency\":\"MXN\"}}")
echo "    $r"
case "$r" in *compensada*) : ;; *) echo "  FALLO: no compenso"; exit 1 ;; esac
# El invariante, no el conteo: de este pedido no queda NINGUN cobro en pie, y
# al comercio no se le pago nada.
en_pie=$(sql_pagos -c "SELECT count(*) FROM payment WHERE order_id = '$ROTA' AND status <> 'refunded'")
reembolsados=$(sql_pagos -c "SELECT count(*) FROM payment WHERE order_id = '$ROTA' AND status = 'refunded'")
pagados=$(sql_pagos -c "SELECT count(*) FROM payout p JOIN payment m ON m.id = p.payment_id WHERE m.order_id = '$ROTA'")
if [ "$en_pie" -eq 0 ] && [ "$reembolsados" -ge 1 ] && [ "$pagados" -eq 0 ]; then
  echo "  OK: el cobro se deshizo y al comercio no se le pago"
else
  echo "  FALLO: en_pie=$en_pie reembolsados=$reembolsados pagados=$pagados"
  exit 1
fi
# y el diario lo dice
estado=$(sql -c "SELECT estado FROM saga_compra ORDER BY actualizado DESC LIMIT 1")
[ "$estado" = "compensada" ] || { echo "  FALLO: el diario dice '$estado'"; exit 1; }

# --- el retome, que es lo que el diario hace posible ---------------------
# Una saga que arranco en otro proceso: el paso 1 quedo `hecho` y el proceso
# murio antes del paso 2. Lo unico que queda del paso 1 es lo que el diario
# guardo, y de ahi tiene que salir el paymentId para compensar.
echo "  una saga colgada en otro proceso, retomada por el barrido"
ORDEN=$(uuid)
cobro=$(curl -sS --fail-with-body -m 30 -X POST "$PAGOS/v1/payments" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDEN\",\"amount\":{\"amount\":$((TOPE + 1)),\"currency\":\"MXN\"}}")
pago=$(printf '%s' "$cobro" | sed 's/.*"paymentId":"\([^"]*\)".*/\1/')
[ -n "$pago" ] || { echo "  FALLO: no se pudo cobrar para el montaje"; exit 1; }

SAGA=$(sql -c "SELECT gen_random_uuid()")
sql -v ON_ERROR_STOP=1 -c "INSERT INTO saga_compra (id, paso, estado, datos, salidas, actualizado)
  VALUES ('$SAGA', 1, 'hecho',
    '{\"id\":\"$SAGA\",\"type\":\"POST /v1/checkouts\",\"source\":\"demo\",\"time\":\"2026-01-01T00:00:00Z\",
      \"traceparent\":\"00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01\",
      \"correlationId\":\"$SAGA\",\"causationId\":null,
      \"data\":{\"orderId\":\"$ORDEN\",\"amount\":{\"amount\":$((TOPE + 1)),\"currency\":\"MXN\"}}}'::jsonb,
    jsonb_build_object('1', jsonb_build_object('paymentId', '$pago')),
    now() - interval '10 minutes')" > /dev/null

barrido=$(curl -sS --fail-with-body -m 90 -X POST "$CHECKOUT/internal/saga/compra/barrer")
echo "    $barrido"
case "$barrido" in
  *'"compensadas":1'*) : ;;
  *) echo "  FALLO: el barrido no compenso la colgada"; exit 1 ;;
esac
# el reembolso salio del paymentId que el DIARIO guardo, no de una variable
estado=$(sql_pagos -c "SELECT status FROM payment WHERE id = '$pago'")
[ "$estado" = "refunded" ] || { echo "  FALLO: el pago quedo en '$estado'"; exit 1; }
final=$(sql -c "SELECT estado FROM saga_compra WHERE id = '$SAGA'")
[ "$final" = "compensada" ] || { echo "  FALLO: el diario dice '$final'"; exit 1; }
echo "  OK: retomada desde el diario, compensada, y el reembolso alcanzo al cobro"

# y no la vuelve a tomar
otra=$(curl -sS -m 60 -X POST "$CHECKOUT/internal/saga/compra/barrer")
case "$otra" in
  *'"reclamadas":0'*) echo "  OK: una saga cerrada no se vuelve a barrer" ;;
  *) echo "  FALLO: el barrido reclamo una saga ya cerrada: $otra"; exit 1 ;;
esac
