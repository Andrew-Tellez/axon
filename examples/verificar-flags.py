#!/usr/bin/env python3
"""Comprueba que el rollout declarado sea el que flagd aplica.

Dos propiedades, y la segunda es la que importa: el porcentaje se acerca al
declarado, y la MISMA entidad recibe siempre la misma respuesta. Sin lo
segundo, un pago tomaria el camino nuevo en una llamada y el viejo en la
siguiente, y quedaria a medio migrar.
"""
import json
import sys
import urllib.request

host = sys.argv[1] if len(sys.argv) > 1 else "localhost:8016"
flag = sys.argv[2] if len(sys.argv) > 2 else "cobro_v2"
esperado = float(sys.argv[3]) if len(sys.argv) > 3 else 10.0
n = 300


def evaluar(tenant):
    # OFREP: el protocolo REST estandar de OpenFeature, no la API propia de flagd
    req = urllib.request.Request(
        f"http://{host}/ofrep/v1/evaluate/flags/{flag}",
        data=json.dumps({"context": {"tenant_id": tenant}}).encode(),
        headers={"content-type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=5) as r:
        return bool(json.load(r).get("value"))


inquilinos = [f"inquilino-{i}" for i in range(n)]
primera = {t: evaluar(t) for t in inquilinos}
encendidos = sum(primera.values())
medido = encendidos * 100 / n

# la misma entidad, otra vez: si cambia, el rollout no es fijo
inestables = [t for t in inquilinos[:40] if evaluar(t) != primera[t]]

print(f"  declarado {esperado:.0f}%  medido {medido:.1f}%  ({encendidos} de {n})")
assert not inestables, f"el rollout no es estable para: {inestables[:5]}"
# margen amplio a proposito: con 300 muestras la varianza es real, y lo que se
# comprueba es que el porcentaje se aplique, no la calidad del hash
assert abs(medido - esperado) < max(5.0, esperado * 0.6), (
    f"el rollout medido ({medido:.1f}%) no se parece al declarado ({esperado:.0f}%)"
)
print(f"  OK: estable por inquilino, y el porcentaje aplica")
