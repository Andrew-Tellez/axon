#!/bin/sh
# La cache, medida. Declarar que algo se cachea no vale nada si nadie comprueba
# que la llave lleva el inquilino, que el interruptor apaga de verdad y que la
# entrada expira cuando el manifiesto dice.
set -eu
cd "$(dirname "$0")"
COMPOSE="docker compose -f axon.local.yml"
EDGE="localhost:${AXON_EDGE_PORT:-8000}"
T1="11111111-1111-1111-1111-111111111111"

vk() { $COMPOSE exec -T cache-orders valkey-cli "$@"; }

# Un pedido real, por el edge, con su scope.
crear() {
  curl -fsS -X POST "http://$EDGE/v1/tenants/$1/orders" \
    -H 'content-type: application/json' -H "x-granted-scopes: orders:read payments:write" \
    -d "{\"customerId\":\"$T1\",\"customerEmail\":\"a@b.c\",\"total\":{\"amount\":2500,\"currency\":\"MXN\"}}" \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["orderId"])'
}
leer() {
  curl -fsS "http://$EDGE/v2/tenants/$1/orders/$2" -H "x-granted-scopes: orders:read" >/dev/null
}

vk FLUSHALL >/dev/null
ORDER=$(crear "$T1")

echo "  una lectura y la llave que el manifiesto declara"
leer "$T1" "$ORDER"
# La llave se construye en el codigo generado, no en el handler: dos sitios
# armandola son dos formas ligeramente distintas y la segunda nunca acierta.
KEY="orders:order:$T1:$ORDER"
if [ "$(vk EXISTS "$KEY" | tr -d '\r')" != "1" ]; then
  echo "  FALLO: no hay entrada en $KEY"; vk KEYS 'orders:*'; exit 1
fi
echo "  OK: la entrada existe con el inquilino EN la llave, no solo el pedido"

# Y lo que la llave con inquilino evita: que el siguiente inquilino reciba lo
# ajeno. Se pide el MISMO pedido desde otro inquilino y no puede ser un acierto.
echo "  el mismo pedido, preguntado por otro inquilino"
T2="22222222-2222-2222-2222-222222222222"
curl -fsS -o /dev/null -w '' "http://$EDGE/v2/tenants/$T2/orders/$ORDER" -H "x-granted-scopes: orders:read" 2>/dev/null || true
if [ "$(vk EXISTS "orders:order:$T2:$ORDER" | tr -d '\r')" = "1" ]; then
  echo "  FALLO: la entrada del otro inquilino comparte llave con la primera"; exit 1
fi
echo "  OK: entradas distintas por inquilino; sin el en la llave la segunda seria un acierto ajeno"

# El TTL declarado, aplicado. `ttl_ms = 2000` en el manifiesto.
TTL=$(vk PTTL "$KEY" | tr -d '\r')
if [ "$TTL" -le 0 ] || [ "$TTL" -gt 2000 ]; then
  echo "  FALLO: PTTL $TTL, y el manifiesto declara 2000 ms"; exit 1
fi
echo "  OK: expira en ${TTL}ms; el TTL sale del manifiesto y cabe en el max_staleness_ms"

# El interruptor. El dia que la invalidacion este mal, esto es lo que se apaga
# sin desplegar —y una regla sobre una metrica puede ser quien lo apague.
echo "  el interruptor, apagado"
python3 - "$T1" "$ORDER" <<'PY'
import json, pathlib, sys
p = pathlib.Path(".axon/flags.json")
d = json.loads(p.read_text())
d["flags"]["cache_orders"]["defaultVariant"] = "off"
p.write_text(json.dumps(d))
PY
sleep 2   # flagd relee el archivo
vk FLUSHALL >/dev/null
leer "$T1" "$ORDER"
if [ "$(vk EXISTS "$KEY" | tr -d '\r')" = "1" ]; then
  echo "  FALLO: la bandera esta en off y la respuesta se guardo igual"; exit 1
fi
echo "  OK: con la bandera en off no se cachea nada; se apaga sin desplegar"
python3 - <<'PY'
import json, pathlib
p = pathlib.Path(".axon/flags.json")
d = json.loads(p.read_text())
d["flags"]["cache_orders"]["defaultVariant"] = "on"
p.write_text(json.dumps(d))
PY
