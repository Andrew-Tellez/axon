#!/bin/sh
# Quien puede llamar a que, medido.
#
# `auth = "required"` dice que quien llama es alguien. Un scope dice que es
# alguien con permiso para ESTO, que es otra pregunta: sin el, cualquier token
# valido —incluido uno emitido para leer— puede devolver dinero.
#
# El gateway valida el token; el servicio decide si lo concedido cubre lo que el
# manifiesto exige. La lista no se reescribe en el handler: viene generada.
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
COMPOSE="docker compose -f axon.local.yml"
P=$($COMPOSE port payments 8080)
PAY='{"paymentId":"11111111-1111-4111-8111-111111111111","amount":{"amount":10,"currency":"MXN"}}'

# lo declarado sale del codigo generado, no de un texto escrito aca
declarado=$(sed -n 's/.*payoutMerchant: \[\(.*\)\],/\1/p' services/payments/contracts.ts | head -1)
echo "  declarado en el codigo generado: payoutMerchant exige $declarado"

body=$(curl -sS -m 30 -X POST "$P/v1/payouts" -H 'content-type: application/json' -d "$PAY")
code=$(curl -sS -m 30 -o /dev/null -w '%{http_code}' -X POST "$P/v1/payouts" -H 'content-type: application/json' -d "$PAY")
echo "    sin el scope: HTTP $code  $body"
case "$code:$body" in
  403:*insufficient_scope*missing*)
    echo "  OK: 403 insufficient_scope, y nombra el que falta —un 403 sin razon es un ticket" ;;
  *) echo "  FALLO: no rechazo la llamada sin permiso"; exit 1 ;;
esac

code=$(curl -sS -m 30 -o /dev/null -w '%{http_code}' -X POST "$P/v1/payouts" \
  -H 'content-type: application/json' -H 'x-granted-scopes: payments:write' -d "$PAY")
echo "    con el scope: HTTP $code"
if [ "$code" = "200" ]; then
  echo "  OK: el mismo llamado pasa con lo que el manifiesto exige, y nada mas cambio"
else
  echo "  FALLO: el scope correcto no alcanzo (HTTP $code)"; exit 1
fi

# Y un scope que alcanza para leer no alcanza para mover dinero: es justo la
# distincion que `required` a secas no puede hacer.
code=$(curl -sS -m 30 -o /dev/null -w '%{http_code}' -X POST "$P/v1/payouts" \
  -H 'content-type: application/json' -H 'x-granted-scopes: orders:read' -d "$PAY")
echo "    con un scope de lectura: HTTP $code"
if [ "$code" = "403" ]; then
  echo "  OK: un token de lectura no mueve dinero. Con solo \`required\`, si podria"
else
  echo "  FALLO: un scope de lectura basto para un pago (HTTP $code)"; exit 1
fi
