#!/bin/sh
# Event sourcing y CQRS, medidos contra Postgres.
#
# Dos afirmaciones que solo prueba un test de verdad:
#   * el UNIQUE (stream_id, version) ES la concurrencia optimista: dos
#     escrituras a la misma version tienen que dejar UNA sola.
#   * la vista es una proyeccion, no una segunda fuente de verdad: tiene que
#     coincidir con lo que sale de reconstruir el flujo.
set -eu
cd "$(dirname "$0")"
COMPOSE="docker compose -f axon.local.yml"
CHECKOUT="localhost:${AXON_PORT_checkout:-8081}"

sql() { $COMPOSE exec -T db-checkout env PGPASSWORD=local psql -qtAX -U postgres -d checkout "$@"; }

echo "  una compra: el flujo, no una fila"
r=$(curl -sS --fail-with-body -m 60 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d '{"orderId":"11111111-1111-4111-8111-111111111111","amount":{"amount":2500,"currency":"MXN"}}')
echo "    $r"
STREAM=$(sql -c "SELECT stream_id FROM compra_event ORDER BY en DESC LIMIT 1" | tr -d ' \r\n')
eventos=$(sql -c "SELECT string_agg(type, ' -> ' ORDER BY version) FROM compra_event WHERE stream_id = '$STREAM'")
echo "    $eventos"
[ -n "$eventos" ] || { echo "  FALLO: el flujo quedo vacio"; exit 1; }

# --- la concurrencia optimista, medida -----------------------------------
# Dos INSERT a la MISMA version, a la vez. Sin el UNIQUE entran los dos, nadie
# ve un error, y el estado reconstruido depende de en que orden se lean.
echo "  dos escrituras concurrentes a la misma version"
ultima=$(sql -c "SELECT max(version) FROM compra_event WHERE stream_id = '$STREAM'" | tr -d ' \r\n')
siguiente=$((ultima + 1))
uno=/tmp/axon-es-1.log
dos=/tmp/axon-es-2.log
ins="INSERT INTO compra_event (id, stream_id, version, type, data)
     VALUES (gen_random_uuid(), '$STREAM', $siguiente, 'compra.compensada@v1', '{\"streamId\":\"$STREAM\",\"motivo\":\"carrera\"}'::jsonb)"
# el `pg_sleep` dentro de la transaccion las solapa a proposito
sql -v ON_ERROR_STOP=1 -c "BEGIN; SELECT pg_sleep(0.4); $ins; COMMIT;" > "$uno" 2>&1 &
p1=$!
sql -v ON_ERROR_STOP=1 -c "BEGIN; SELECT pg_sleep(0.4); $ins; COMMIT;" > "$dos" 2>&1 &
p2=$!
ok=0
wait $p1 && ok=$((ok + 1)) || true
wait $p2 && ok=$((ok + 1)) || true
filas=$(sql -c "SELECT count(*) FROM compra_event WHERE stream_id = '$STREAM' AND version = $siguiente" | tr -d ' \r\n')
if [ "$ok" -eq 1 ] && [ "$filas" -eq 1 ]; then
  echo "  OK: una entro y la otra fue rechazada; queda 1 evento en la version $siguiente"
  grep -q "duplicate key" "$dos" "$uno" && echo "  i el rechazo es el UNIQUE, no una comprobacion de la aplicacion" || true
else
  echo "  FALLO: $ok escrituras exitosas y $filas filas en la version $siguiente"
  cat "$uno" "$dos"
  exit 1
fi

# --- la vista concuerda con el flujo -------------------------------------
# Es la unica forma de saber que la proyeccion no se quedo a medias: si la vista
# y el fold dicen distinto, el modelo de lectura esta mintiendo.
echo "  la vista contra el estado reconstruido del flujo"
# el ultimo evento del flujo manda: el estado que la vista deberia mostrar
esperado=$(sql -c "SELECT replace(split_part(type, '.', 2), '@v1', '')
                     FROM compra_event WHERE stream_id = '$STREAM'
                    ORDER BY version DESC LIMIT 1" | tr -d ' \r\n')
en_vista=$(sql -c "SELECT estado FROM vista_conversion WHERE stream_id = '$STREAM'" | tr -d ' \r\n')
# la carrera anterior anoto un evento que la vista no vio: se reproyecta
posicion=$(sql -c "SELECT posicion FROM vista_conversion_checkpoint WHERE vista = 'conversion'" | tr -d ' \r\n')
echo "    flujo dice '$esperado', vista dice '$en_vista', checkpoint en $posicion"
if [ "$esperado" = "$en_vista" ]; then
  echo "  OK: la vista concuerda con el flujo"
else
  # No es un fallo del sistema: la carrera de arriba escribio DIRECTO al flujo,
  # sin pasar por la proyeccion. Lo que importa es que el checkpoint lo diga.
  ultima=$(sql -c "SELECT max(version) FROM compra_event WHERE stream_id = '$STREAM'" | tr -d ' \r\n')
  if [ "$posicion" -lt "$ultima" ]; then
    echo "  OK: la vista esta atrasada Y el checkpoint lo dice ($posicion de $ultima)"
    echo "  i un evento escrito al flujo sin pasar por la proyeccion deja la vista"
    echo "    atras, y el checkpoint es lo unico que permite saberlo y retomar"
  else
    echo "  FALLO: la vista dice '$en_vista' y el checkpoint esta al dia en $posicion"
    exit 1
  fi
fi

# --- el atraso, contra el presupuesto declarado --------------------------
# El atraso de una vista NO es la edad del evento que ya aplico: es cuanto lleva
# sin ver lo que ya ocurrio. Medirlo de la vista da siempre un numero bonito
# —justo cuando esta parada— asi que sale del FLUJO: la edad del evento mas
# viejo que la proyeccion todavia no aplico.
echo "  el atraso de la vista contra su presupuesto"
tope=$(sed -n 's/.*conversionAtrasoMaximoMs = \([0-9]*\).*/\1/p' services/checkout/contracts.ts | head -1)
atraso() {
  sql -c "SELECT coalesce(max(extract(epoch from (now() - e.en)) * 1000)::bigint, 0)
            FROM compra_event e
            LEFT JOIN vista_conversion_checkpoint c ON c.vista = 'conversion'
           WHERE e.stream_id = '$1' AND e.version > coalesce(c.posicion, 0)" | tr -d ' \r\n'
}

# En el flujo de arriba quedo un evento SIN proyectar —lo escribio la carrera,
# sin pasar por la proyeccion— asi que la medicion tiene que verlo.
atrasado=$(atraso "$STREAM")
if [ "$atrasado" -gt 0 ]; then
  echo "  OK: ${atrasado}ms de atraso real; el evento sin proyectar se ve"
else
  echo "  FALLO: hay un evento sin proyectar y el atraso medido es 0"
  exit 1
fi

# Y una compra nueva, con la vista al dia, tiene que caber en el presupuesto.
# Sin este segundo caso, la medicion podria estar dando siempre un numero alto.
echo "  y una compra al dia, dentro del presupuesto"
curl -sS --fail-with-body -m 60 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d '{"orderId":"44444444-4444-4444-8444-444444444444","amount":{"amount":900,"currency":"MXN"}}' > /dev/null
NUEVO=$(sql -c "SELECT stream_id FROM compra_event ORDER BY en DESC LIMIT 1" | tr -d ' \r\n')
al_dia=$(atraso "$NUEVO")
if [ "$al_dia" -le "$tope" ]; then
  echo "  OK: ${al_dia}ms, dentro del presupuesto declarado de ${tope}ms"
else
  echo "  FALLO: ${al_dia}ms contra un presupuesto de ${tope}ms"
  exit 1
fi
