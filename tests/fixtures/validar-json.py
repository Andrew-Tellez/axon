#!/usr/bin/env python3
"""Valida un JSON contra un JSON Schema.

Se usa para el plan neutral: el esquema esta escrito a mano, asi que lo unico
que lo mantiene honesto es que un plan REAL pase por un validador de verdad.
Como cada objeto del esquema rechaza lo que no declara, la deriva se atrapa en
los dos sentidos: un campo nuevo en el plan que no este en el esquema falla, y
uno declarado en el esquema que el plan no tenga falla tambien.
"""
import json
import sys

try:
    import jsonschema
except ImportError:
    print("SALTEADO: falta el modulo jsonschema")
    sys.exit(0)

doc_path, esquema_path = sys.argv[1], sys.argv[2]
doc = json.load(open(doc_path))
esquema = json.load(open(esquema_path))

validador = jsonschema.Draft202012Validator(esquema)
errores = sorted(validador.iter_errors(doc), key=lambda e: list(e.path))
if errores:
    for e in errores[:10]:
        ruta = "/".join(str(p) for p in e.path) or "(raiz)"
        print(f"FALLO en {ruta}: {e.message}")
    sys.exit(1)
print(f"OK: valida contra {esquema_path}")
