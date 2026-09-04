#!/usr/bin/env python3
"""Valida un pgdog.toml contra el JSON Schema oficial de pgdog.

Validar contra su esquema —generado desde sus propios tipos de Rust y
comprobado por su CI— es validar contra el parser real que va a leer el
archivo, no contra nuestra idea de como deberia ser su configuracion.
"""
import json
import re
import sys
import tomllib

try:
    import jsonschema
except ImportError:
    print("SALTEADO: falta el modulo jsonschema")
    sys.exit(0)

cfg_path, esquema_path = sys.argv[1], sys.argv[2]
txt = open(cfg_path).read()

# Los ${VAR} son marcadores para el despliegue, no TOML: un archivo generado no
# es lugar para un host ni una contrasena.
txt = re.sub(r"port = \$\{[^}]+\}", "port = 5432", txt)
txt = re.sub(r"\$\{[^}]+\}", "host.interno", txt)

cfg = tomllib.loads(txt)
jsonschema.validate(cfg, json.load(open(esquema_path)))
print(f"OK: valida contra el esquema oficial. nodos={len(cfg['databases'])} "
      f"repartidas={len(cfg.get('sharded_tables', []))}")
