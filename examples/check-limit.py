#!/usr/bin/env python3
"""Lo que la rampa existe para distinguir.

Un 429 es el limite declarado funcionando; un 5xx es el servicio rompiendose.
k6 cuenta los dos como `http_req_failed`, asi que el script generado los separa
en dos metricas propias y esto las lee.
"""
import json, sys

m = json.load(open(sys.argv[1]))["metrics"]
thr = m.get("throttled", {}).get("value", 0)
err = m.get("server_errors", {}).get("value", 0)
ok = m.get("checks", {}).get("value", 0)
print(f"    throttled {thr*100:.0f}%  ·  5xx {err*100:.0f}%  ·  respuestas esperadas {ok*100:.0f}%")
if thr > 0 and err == 0 and ok == 1:
    print("  OK: pasado su limite degrada con 429 y no se cae con 500, que es para lo que se declara un limite")
else:
    sys.exit(f"  FALLO: throttled={thr} 5xx={err} checks={ok}")
