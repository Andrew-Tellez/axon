#!/bin/sh
# La cadena REAL, leida del almacen de trazas y no del log de axon.
#
# Es lo que hace que esto sirva en un repo que nunca adopto el envelope: un span
# ES un envelope con otros nombres —spanId el id, parentSpanId la causa, traceId
# el flujo—, y cualquier repo con OpenTelemetry ya lo tiene sin decidir nada.
#
# Y contesta la mitad que el edge no puede ver: una llamada entre servicios no
# pasa por ahi.
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
UI="localhost:${AXON_TRACE_UI_PORT:-16686}"

curl -sS --fail-with-body -m 30 "http://$UI/api/traces?service=orders&limit=40" > .axon/jaeger.json
spans=$(python3 -c 'import json;d=json.load(open(".axon/jaeger.json"));print(sum(len(t["spans"]) for t in d.get("data",[])))')
echo "  $spans spans leidos de Jaeger, sin tocar el log de envelopes"

out=$("$AXON" trace .axon/jaeger.json --manifests .)
echo "$out" | grep -E "^(read from|[0-9]+ edges|  (ok|undeclared|outside|quiet))" | head -8 | sed 's/^/    /'

case "$out" in
  *"read from the Jaeger spans"*) : ;;
  *) echo "  FALLO: no reconocio el formato de Jaeger"; exit 1 ;;
esac
# La cadena real cruzando servicios: es lo mismo que el log de envelopes dice,
# leido de otra fuente. Si las dos no coincidieran, una de las dos miente.
case "$out" in
  *"ok orders → payments"*)
    echo "  OK: la arista orders → payments esta en las trazas y esta declarada" ;;
  *) echo "  FALLO: no vio la arista que el demo acaba de producir"; exit 1 ;;
esac

# Y lo que el edge no puede ver: una arista entre servicios que nadie declara.
# Se prueba con una traza inventada, porque el ejemplo no tiene esa deriva.
python3 - <<'PY'
import json, pathlib
spans = {
  "data": [{
    "traceID": "t1",
    "spans": [
      {"traceID":"t1","spanID":"a1","operationName":"POST /v1/checkouts","processID":"p1","references":[],"startTime":1},
      {"traceID":"t1","spanID":"b1","operationName":"GET /v1/tenants/{tenantId}/orders/{orderId}","processID":"p2",
       "references":[{"refType":"CHILD_OF","spanID":"a1"}],"startTime":2}
    ],
    "processes": {"p1":{"serviceName":"checkout"},"p2":{"serviceName":"orders"}}
  }]
}
pathlib.Path(".axon/deriva.json").write_text(json.dumps(spans))
PY
code=0
out=$("$AXON" trace .axon/deriva.json --manifests .) || code=$?
echo "$out" | grep -E "undeclared|^fail" | head -2 | sed 's/^/    /'
if [ "$code" -ne 0 ] && printf '%s' "$out" | grep -q "undeclared checkout → orders"; then
  echo "  OK: una dependencia que ocurre y nadie declara falla, y el edge no la habria visto"
else
  echo "  FALLO: no detecto la arista no declarada (code=$code)"
  exit 1
fi
rm -f .axon/deriva.json
