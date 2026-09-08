#!/usr/bin/env python3
"""El lazo cerrado: la regla mueve la palanca, y flagd sirve lo movido.

Dos cerrojos y no uno: el manifiesto dice que la regla PUEDE aplicarse
(`mode = "apply"`) y quien corre esto dice que ahora (`--apply`). Ninguno solo
hace nada, porque contestan preguntas distintas.

Lo que se mide no es que el archivo cambie —eso seria comprobar que un `write`
escribe—: es que flagd, el que responde a los servicios, sirva la variante nueva.
"""
import json, subprocess, sys, time, urllib.error, urllib.request

AXON = "../target/release/axon"
FLAGS = ".axon/flags.json"
HOST = sys.argv[1] if len(sys.argv) > 1 else "localhost:8016"
FLAG = "free_shipping"


def ch(sql):
    return subprocess.run(
        ["docker", "compose", "-f", "axon.local.yml", "exec", "-T", "warehouse",
         "clickhouse-client", "--user", "local", "--password", "local", "-q", sql],
        capture_output=True, text=True, timeout=120, stdin=subprocess.DEVNULL, check=True).stdout.strip()


def seed(day, amount):
    ch(f"""INSERT INTO axon.order_placed_v1
           (event_id, event_type, source, event_time, correlation_id, order_id, total_amount, total_currency)
           VALUES (generateUUIDv4(),'order.placed@v1','apply-demo',now() - INTERVAL {day} DAY,
                   generateUUIDv4(),generateUUIDv4(),{amount},'USD'),
                  (generateUUIDv4(),'order.placed@v1','apply-demo',now() - INTERVAL {day} DAY,
                   generateUUIDv4(),generateUUIDv4(),{amount},'USD')""")


def windows():
    sql = open(".axon/rules.sql").read()
    out = subprocess.run(
        ["docker", "compose", "-f", "axon.local.yml", "exec", "-T", "warehouse",
         "clickhouse-client", "--user", "local", "--password", "local", "--multiquery", "--format", "TSV"],
        input=sql, capture_output=True, text=True, timeout=180, check=True).stdout
    open(".axon/rules.tsv", "w").write(out)


def rules(*extra):
    return subprocess.run([AXON, "rules", ".", "--check", ".axon/rules.tsv", *extra],
                          capture_output=True, text=True, timeout=120,
                          stdin=subprocess.DEVNULL, check=True).stdout


def served():
    """Lo que flagd le contesta a un servicio. flagd relee el archivo solo."""
    for _ in range(40):
        try:
            req = urllib.request.Request(
                f"http://{HOST}/ofrep/v1/evaluate/flags/{FLAG}",
                data=json.dumps({"context": {"tenant_id": "t"}}).encode(),
                headers={"Content-Type": "application/json"})
            with urllib.request.urlopen(req, timeout=5) as r:
                return json.load(r)["variant"]
        except Exception:
            time.sleep(0.5)
    sys.exit("  FALLO: flagd no contesta")


# Las dos etiquetas: `check-rules.sh` siembra con la suya y las dos series se
# sumarian en el mismo dia, que es como una caida deja de parecer una caida.
ch("ALTER TABLE axon.order_placed_v1 DELETE WHERE source IN ('apply-demo','rules-demo') SETTINGS mutations_sync = 2")
subprocess.run([AXON, "flags", "."], stdout=open(FLAGS, "w"), check=True)
subprocess.run(f"{AXON} rules . | sed 's/\"@dataset\\.\\([a-z0-9_]*\\)\"/axon.\\1/g' > .axon/rules.sql",
               shell=True, check=True)
time.sleep(1)
print(f"  flagd sirve `{FLAG}` = {served()} antes de nada")

# --- el negocio cae ------------------------------------------------------
for d in (6, 5, 4, 3):
    seed(d, 50000)
seed(2, 40000)
seed(1, 32000)
windows()

# Un cerrojo solo no hace nada: la regla dice `apply` y aun asi, sin la bandera,
# solo propone.
out = rules()
if "none was applied" not in out:
    sys.exit("  FALLO: aplico sin que nadie se lo pidiera")
print("  OK: la regla dice `apply` y sin la bandera igual solo propone —dos cerrojos, no uno")

out = rules("--apply", FLAGS)
print("   ", [l for l in out.splitlines() if l.startswith("applied")][0])
time.sleep(2)
now = served()
print(f"    flagd sirve `{FLAG}` = {now}")
if now != "over_500":
    sys.exit(f"  FALLO: flagd sigue sirviendo {now}")
print("  OK: la palanca se movio y flagd sirve la variante nueva a quien pregunte")

# --- y el negocio se recupera -------------------------------------------
# Las dos etiquetas: `check-rules.sh` siembra con la suya y las dos series se
# sumarian en el mismo dia, que es como una caida deja de parecer una caida.
ch("ALTER TABLE axon.order_placed_v1 DELETE WHERE source IN ('apply-demo','rules-demo') SETTINGS mutations_sync = 2")
for d in (6, 5, 4, 3):
    seed(d, 50000)
seed(2, 52000)
seed(1, 54000)
windows()
out = rules("--apply", FLAGS)
time.sleep(2)
now = served()
print(f"    flagd sirve `{FLAG}` = {now}")
if now != "off":
    sys.exit(f"  FALLO: la palanca se quedo en {now}; lo que sube solo tiene que bajar solo")
print("  OK: al levantarse la condicion vuelve sola. Sin esto la palanca se queda donde la dejo el peor dia del trimestre")

# --- y el rastro ---------------------------------------------------------
trail = [json.loads(l) for l in open(".axon/flags.audit.ndjson")]
for t in trail[-2:]:
    print(f"    auditoria: {t['at']} {t['flag']} {t['from']} -> {t['to']}")
if len(trail) < 2 or trail[-1]["to"] != "off" or trail[-2]["to"] != "over_500":
    sys.exit(f"  FALLO: la auditoria no cuenta las dos: {trail}")
print("  OK: las dos quedaron escritas con lo que la regla leyo. Un cambio automatico sin rastro es la peor version de esto")
# Las dos etiquetas: `check-rules.sh` siembra con la suya y las dos series se
# sumarian en el mismo dia, que es como una caida deja de parecer una caida.
ch("ALTER TABLE axon.order_placed_v1 DELETE WHERE source IN ('apply-demo','rules-demo') SETTINGS mutations_sync = 2")
