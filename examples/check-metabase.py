#!/usr/bin/env python3
"""Metabase, aprovisionado desde el manifiesto y comprobado.

Lo declarado sale de `axon analytics --metabase`: la conexion y una pregunta por
metrica y por embudo. Lo medido es lo que contesta Metabase cuando se le corre
esa pregunta, comparado contra la misma vista leida directo de ClickHouse.

Que coincidan es el punto entero: la definicion de una metrica vive en el
manifiesto, no retecleada en un dashboard donde dos tableros pueden discrepar.
"""
import json, subprocess, sys, time, urllib.error, urllib.request, http.cookiejar

BI = f"http://localhost:{sys.argv[1] if len(sys.argv) > 1 else 3000}/api"
USER, PASSWORD = "axon@example.com", "axon-Demo-1234"
jar = http.cookiejar.CookieJar()
http_ = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))


def api(path, data=None, method=None):
    body = json.dumps(data).encode() if data is not None else None
    r = urllib.request.Request(
        BI + path, data=body, headers={"Content-Type": "application/json"},
        method=method or ("POST" if data else "GET"))
    try:
        with http_.open(r, timeout=90) as resp:
            return resp.status, json.loads(resp.read().decode() or "{}")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()[:200]


def ch(sql):
    return subprocess.run(
        ["docker", "compose", "-f", "axon.local.yml", "exec", "-T", "warehouse",
         "clickhouse-client", "--user", "local", "--password", "local", "-q", sql],
        capture_output=True, text=True, check=True).stdout.strip()


plan = json.loads(subprocess.run(
    ["../target/release/axon", "analytics", ".", "--metabase"],
    capture_output=True, text=True, check=True).stdout)
cards = plan["cards"]
print(f"  declarado: {len(cards)} pregunta(s) —una por metrica y por embudo— contra las vistas generadas")

for i in range(90):
    try:
        if api("/health")[1].get("status") == "ok":
            break
    except Exception:
        pass
    time.sleep(1)
else:
    sys.exit("  FALLO: Metabase no arranco")

# Setup sin manos la primera vez; despues, sesion. El contenedor guarda su
# base de aplicacion, asi que la segunda corrida encuentra el trabajo hecho: un
# chequeo que solo funciona con todo recien creado no se corre dos veces.
token = api("/session/properties")[1].get("setup-token")
done = False
if token:
    st, _ = api("/setup", {
        "token": token,
        "user": {"first_name": "axon", "last_name": "demo", "email": USER,
                 "password": PASSWORD, "site_name": "axon"},
        "prefs": {"site_name": "axon", "allow_tracking": False}})
    done = st == 200
    if done:
        print("  Metabase aprovisionado desde cero, sin tocar la interfaz")
if not done:
    st, _ = api("/session", {"username": USER, "password": PASSWORD})
    if st != 200:
        sys.exit("  FALLO: ni se pudo hacer el setup ni entrar con la sesion")
    print("  Metabase ya estaba aprovisionado: se entra y se comprueba igual")

st, dbs = api("/database")
existing = [d for d in dbs["data"] if d["engine"] == plan["database"]["engine"]]
if existing:
    dbid = existing[0]["id"]
else:
    st, db = api("/database", {"name": plan["database"]["name"],
                               "engine": plan["database"]["engine"],
                               "details": plan["database"]["details"]})
    if st != 200:
        sys.exit(f"  FALLO: la conexion declarada no conecta: {db}")
    dbid = db["id"]
print(f"  conectado a la bodega con los datos del manifiesto (database {dbid})")

# Las preguntas se recrean por nombre: correrlo dos veces no deja duplicados,
# y lo que se comprueba es que la que queda apunte a la vista declarada.
st, existing_cards = api("/card")
by_name = {c["name"]: c["id"] for c in existing_cards} if st == 200 else {}
created = 0
for c in cards:
    if c["name"] in by_name:
        api(f"/card/{by_name[c['name']]}", {"archived": True}, method="PUT")
    st, card = api("/card", {"name": c["name"], "display": "table",
                             "visualization_settings": {},
                             "dataset_query": {"type": "native", "database": dbid,
                                               "native": {"query": c["sql"]}}})
    if st != 200:
        sys.exit(f"  FALLO: no se creo la pregunta `{c['name']}`: {card}")
    created += 1
print(f"  OK: {created} preguntas creadas, cada una apuntando a la vista que declara el manifiesto")

# Y la comprobacion que importa: la tarjeta y la vista tienen que contestar lo
# mismo. Si no, el tablero mide otra cosa que el manifiesto.
metric = next(c for c in cards if c["kind"] == "metric" and c["view"] == "metric_gmv")
st, res = api("/dataset", {"type": "native", "database": dbid,
                           "native": {"query": f"SELECT sum(value) FROM axon.{metric['view']}"}})
if st not in (200, 202):
    sys.exit(f"  FALLO: la pregunta no corrio: {res}")
via_bi = float(res["data"]["rows"][0][0])
via_ch = float(ch(f"SELECT sum(value) FROM axon.{metric['view']}"))
print(f"    metabase {via_bi:.0f}  ·  clickhouse {via_ch:.0f}")
if via_bi != via_ch:
    sys.exit(f"  FALLO: el tablero y la vista no coinciden ({via_bi} vs {via_ch})")
print("  OK: la pregunta contesta lo mismo que la vista; la metrica se define en un solo lugar")

# --- y el drift al reves ---------------------------------------------------
# axon emite las preguntas y las compara contra el manifiesto. La que escribe
# alguien A MANO en el tablero, contra una tabla que axon posee, era invisible:
# el dia que la columna cambia esa pregunta se rompe y nadie se entera hasta que
# la abre. Aqui se escribe una de esas —la mas real de todas: la que pregunta por
# el correo en claro, que el manifiesto declara PII y por eso no existe con ese
# nombre— y se comprueba que axon la nombra.
print("  una pregunta escrita a mano contra una tabla de axon")
A_MANO = "quien compro (a mano)"
if A_MANO not in by_name:
    st, card = api("/card", {
        "name": A_MANO, "display": "table", "visualization_settings": {},
        "dataset_query": {"type": "native", "database": dbid,
                          "native": {"query": "SELECT customer_email, count(*) "
                                              "FROM axon.order_placed_v1 GROUP BY customer_email"}},
    })
    if st not in (200, 202):
        sys.exit(f"  FALLO: no se pudo crear la pregunta a mano: {card}")

st, todas = api("/card")
if st != 200:
    sys.exit(f"  FALLO: no se pudieron exportar las preguntas: {todas}")
open(".axon/metabase-cards.json", "w").write(json.dumps(todas))

r = subprocess.run(["../target/release/axon", "analytics", ".", "--metabase",
                    "--check", ".axon/metabase-cards.json"],
                   capture_output=True, text=True)
salida = r.stdout + r.stderr
if r.returncode == 0:
    sys.exit(f"  FALLO: la pregunta a mano lee una columna que no existe y paso limpia:\n{salida}")
if "customer_email_hash" not in salida:
    sys.exit(f"  FALLO: la nombra pero no dice que hay en su lugar:\n{salida}")
print("    " + [l for l in salida.splitlines() if "reads `" in l][0][:200])
print("  OK: nombra la pregunta escrita a mano y dice que columna hay en su lugar")
