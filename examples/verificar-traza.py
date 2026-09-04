#!/usr/bin/env python3
"""Comprueba la forma de la traza que llego a OpenTelemetry.

Lo que se rompe en cuanto alguien inventa un `traceparent` no es que falte la
traza: es que aparece partida en fragmentos que cuelgan de padres que nunca
existieron, y en la UI eso se ve como varias trazas cortas en vez de una.
"""
import json
import sys
import time
import urllib.parse
import urllib.request

ui = sys.argv[1] if len(sys.argv) > 1 else "localhost:16686"

# Se busca LA traza de esta corrida, no la ultima que haya: el colector
# conserva las anteriores y "la ultima" no es un criterio.
with open(".axon/local.ndjson") as f:
    del_log = json.loads(f.readline())["correlationId"]
filtro = urllib.parse.quote(json.dumps({"axon.correlation_id": del_log}))
url = f"http://{ui}/api/traces?service=orders&tags={filtro}&lookback=1h&limit=5"

# El exportador manda en lotes: la traza tarda en aparecer. Se reintenta aca en
# vez de esperar afuera, porque afuera no hay forma de saber si la que llego es
# la de esta corrida o la de la anterior.
datos = []
for _ in range(60):
    try:
        with urllib.request.urlopen(url) as r:
            datos = json.load(r)["data"]
    except Exception:
        datos = []
    if datos:
        break
    time.sleep(1)

assert datos, f"sin trazas para el flujo {del_log} tras 60s"
assert len(datos) == 1, f"el flujo {del_log} aparece partido en {len(datos)} trazas"
t = datos[0]
spans = {s["spanID"]: s for s in t["spans"]}
servicios = {p["serviceName"] for p in t["processes"].values()}


def padre(s):
    return next((r["spanID"] for r in s.get("references", []) if r["refType"] == "CHILD_OF"), None)


def prof(s):
    p = padre(s)
    return 0 if not p or p not in spans else prof(spans[p]) + 1


for s in sorted(t["spans"], key=lambda s: s["startTime"]):
    proc = t["processes"][s["processID"]]["serviceName"]
    print(f'  {"    " * prof(s)}{proc}/{s["operationName"]}')

raices = [s for s in t["spans"] if not padre(s)]
huerfanos = [s for s in t["spans"] if padre(s) and padre(s) not in spans]
assert len(raices) == 1, f"se esperaba un solo span raiz, hay {len(raices)}"
assert not huerfanos, f"{len(huerfanos)} spans cuelgan de un padre inexistente"
assert servicios >= {"orders", "payments"}, f"la traza no cruzo los servicios: {servicios}"

# el mismo flujo de negocio, visto desde los dos lados
corr = {x["value"] for s in t["spans"] for x in s["tags"] if x["key"] == "axon.correlation_id"}
assert corr == {del_log}, f"la traza mezcla flujos: {corr}"

print(f'  OK: {len(t["spans"])} spans, un raiz, sin huerfanos, cruzando {sorted(servicios)}')
print(f"  y el correlationId coincide con el del log: {del_log}")
