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

# --- el relay: nadie publica en linea -----------------------------------
# El flujo es la verdad y el outbox es la entrega, escritos en UNA transaccion.
# Lo que se mide es lo que eso compra: un evento anotado mientras el relay esta
# caido se publica cuando vuelve, sin que nadie lo reintente a mano.
echo "  un evento anotado con el relay caido"
$COMPOSE stop checkout > /dev/null 2>&1
HUERFANO=$(sql -c "SELECT gen_random_uuid()" | tr -d ' \r\n')
ultima=$(sql -c "SELECT max(version) FROM compra_event WHERE stream_id = '$NUEVO'" | tr -d ' \r\n')
# Las dos filas, en una transaccion, igual que hace `append`.
sql -v ON_ERROR_STOP=1 -c "BEGIN;
  INSERT INTO compra_event (id, stream_id, version, type, data)
  VALUES ('$HUERFANO', '$NUEVO', $((ultima + 1)), 'compra.compensada@v1',
          '{\"streamId\":\"$NUEVO\",\"motivo\":\"relay caido\"}'::jsonb);
  INSERT INTO outbox (id, type, source, time, traceparent, correlation_id, causation_id, data)
  VALUES ('$HUERFANO', 'compra.compensada@v1', 'checkout', now()::text,
          '00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01', '$NUEVO', NULL,
          '{\"streamId\":\"$NUEVO\",\"motivo\":\"relay caido\"}'::jsonb);
  COMMIT;" > /dev/null
pendientes=$(sql -c "SELECT count(*) FROM outbox WHERE published_at IS NULL" | tr -d ' \r\n')
[ "$pendientes" -ge 1 ] || { echo "  FALLO: el evento no quedo pendiente"; $COMPOSE up -d --wait checkout > /dev/null 2>&1; exit 1; }
echo "    $pendientes evento(s) anotado(s) y sin publicar"

$COMPOSE up -d --wait checkout > /dev/null 2>&1
i=0
while [ "$(sql -c "SELECT count(*) FROM outbox WHERE id = '$HUERFANO' AND published_at IS NOT NULL" | tr -d ' \r\n')" -eq 0 ]; do
  i=$((i + 1))
  [ "$i" -gt 30 ] && { echo "  FALLO: el relay volvio y no publico lo pendiente"; exit 1; }
  sleep 1
done
# y llego al bus de verdad, no solo se marco
if grep -q "$HUERFANO" .axon/local.ndjson 2>/dev/null; then
  echo "  OK: el relay volvio y lo publico; nadie tuvo que reintentarlo a mano"
else
  echo "  FALLO: se marco como publicado y no aparece en el log de envelopes"
  exit 1
fi

# --- las fotos: una cache que no puede mentir ---------------------------
# Una foto es una cache del fold. Lo peligroso no es que falte —eso solo cuesta
# tiempo— sino que este MAL: rehidratar de una foto incorrecta da un estado que
# ya no coincide con reproducir el flujo, y eso no da ningun error.
echo "  la foto, en la cadencia declarada"
cada=$(sed -n 's/.*compraFotoCada = \([0-9]*\).*/\1/p' services/checkout/contracts.ts | head -1)
reglas=$(sed -n 's/.*compraFotoReglas = \([0-9]*\).*/\1/p' services/checkout/contracts.ts | head -1)
FOTO=$(sql -c "SELECT stream_id FROM compra_snapshot ORDER BY en DESC LIMIT 1" | tr -d ' \r\n')
[ -n "$FOTO" ] || { echo "  FALLO: snapshot_every declarado y ninguna foto guardada"; exit 1; }
ver=$(sql -c "SELECT version FROM compra_snapshot WHERE stream_id = '$FOTO' ORDER BY version DESC LIMIT 1" | tr -d ' \r\n')
if [ $((ver % cada)) -eq 0 ]; then
  echo "  OK: foto en la version $ver, multiplo de la cadencia declarada ($cada)"
else
  echo "  FALLO: foto en la version $ver y la cadencia declarada es $cada"
  exit 1
fi

# Lo que hace segura a una foto: rehidratar desde ella da EXACTAMENTE lo mismo
# que reproducir el flujo entero. Se compara el estado guardado contra el que
# sale de la vista, que se construyo evento por evento sin usar fotos.
echo "  la foto contra la vista, que se construyo sin fotos"
en_foto=$(sql -c "SELECT estado->>'estado' FROM compra_snapshot WHERE stream_id = '$FOTO' ORDER BY version DESC LIMIT 1" | tr -d ' \r\n')
en_vista=$(sql -c "SELECT estado FROM vista_conversion WHERE stream_id = '$FOTO'" | tr -d ' \r\n')
if [ "$en_foto" = "$en_vista" ]; then
  echo "  OK: la foto dice '$en_foto' y la proyeccion, que no la uso, dice lo mismo"
else
  echo "  FALLO: foto '$en_foto' contra vista '$en_vista'"
  exit 1
fi

# Y la parte que convierte el fallo silencioso en uno que se corrige solo: una
# foto de OTRA version de reglas se ignora. Se ensucia una a proposito y el
# servicio tiene que seguir dando el estado correcto.
echo "  una foto con reglas viejas, envenenada a proposito"
sql -v ON_ERROR_STOP=1 -c "INSERT INTO compra_snapshot (stream_id, version, reglas, estado)
  VALUES ('$FOTO', $ver, $((reglas - 1)),
          '{\"estado\":\"basura\",\"centavos\":-1,\"paymentId\":null}'::jsonb)" > /dev/null
# una compra nueva sobre el MISMO flujo tiene que salir del estado real, no de
# la foto envenenada
ultima=$(sql -c "SELECT max(version) FROM compra_event WHERE stream_id = '$FOTO'" | tr -d ' \r\n')
sql -v ON_ERROR_STOP=1 -c "INSERT INTO compra_event (id, stream_id, version, type, data)
  VALUES (gen_random_uuid(), '$FOTO', $((ultima + 1)), 'compra.compensada@v1',
          '{\"streamId\":\"$FOTO\",\"motivo\":\"prueba de foto\"}'::jsonb)" > /dev/null
buenas=$(sql -c "SELECT count(*) FROM compra_snapshot WHERE stream_id = '$FOTO' AND reglas = $reglas" | tr -d ' \r\n')
malas=$(sql -c "SELECT count(*) FROM compra_snapshot WHERE stream_id = '$FOTO' AND reglas <> $reglas" | tr -d ' \r\n')
centavos=$(sql -c "SELECT (estado->>'centavos')::bigint FROM compra_snapshot
                    WHERE stream_id = '$FOTO' AND reglas = $reglas ORDER BY version DESC LIMIT 1" | tr -d ' \r\n')
if [ "$malas" -ge 1 ] && [ "$buenas" -ge 1 ] && [ "$centavos" -gt 0 ]; then
  echo "  OK: la foto de reglas $((reglas - 1)) convive con la vigente, que dice $centavos centavos"
  echo "  i que el codigo la IGNORE lo prueba el testkit: aqui se comprueba que"
  echo "    una foto de otra version no pisa a la vigente. Subir snapshot_version"
  echo "    es lo que convierte 'la foto quedo mal' en 'la foto se reconstruye'"
else
  echo "  FALLO: buenas=$buenas malas=$malas centavos=$centavos"
  exit 1
fi

# --- la limpieza de fotos -----------------------------------------------
# `snapshot_version` invalida las fotos viejas pero no las quita, asi que la
# tabla crece con cada version de reglas. La limpieza puede ser agresiva porque
# una foto es una cache: lo peor que pasa es reconstruir desde el flujo.
echo "  la limpieza de fotos que la version vigente no usa"
antes=$(sql -c "SELECT count(*) FROM compra_snapshot" | tr -d ' \r\n')
viejas=$(sql -c "SELECT count(*) FROM compra_snapshot WHERE reglas <> $reglas" | tr -d ' \r\n')
[ "$viejas" -ge 1 ] || { echo "  FALLO: el montaje no dejo ninguna foto de otra version"; exit 1; }
borradas=$(curl -sS --fail-with-body -m 30 -X POST "$CHECKOUT/internal/aggregate/compra/limpiar" \
  | sed 's/.*"borradas":\([0-9]*\).*/\1/')
despues=$(sql -c "SELECT count(*) FROM compra_snapshot" | tr -d ' \r\n')
quedan_viejas=$(sql -c "SELECT count(*) FROM compra_snapshot WHERE reglas <> $reglas" | tr -d ' \r\n')
echo "    $antes fotos, borro $borradas, quedan $despues"
if [ "$quedan_viejas" -eq 0 ] && [ "$borradas" -ge "$viejas" ] && [ "$despues" -lt "$antes" ]; then
  echo "  OK: las de otra version se fueron, y de cada flujo queda solo la mas nueva"
else
  echo "  FALLO: quedan $quedan_viejas de otra version; $antes -> $despues, borradas=$borradas"
  exit 1
fi

# Una por flujo como maximo: mas de una es espacio que nadie lee, porque
# `foto()` toma siempre la mas nueva.
duplicadas=$(sql -c "SELECT coalesce(max(n), 0) FROM (
                       SELECT count(*) AS n FROM compra_snapshot GROUP BY stream_id
                     ) t" | tr -d ' \r\n')
[ "$duplicadas" -le 1 ] || { echo "  FALLO: un flujo quedo con $duplicadas fotos"; exit 1; }

# Y lo que hace segura a la limpieza: el estado sigue siendo correcto. La foto
# se rehace en el proximo evento, y mientras tanto se reconstruye del flujo.
echo "  y el estado despues de quedarse sin fotos"
sql -v ON_ERROR_STOP=1 -c "DELETE FROM compra_snapshot" > /dev/null
r=$(curl -sS --fail-with-body -m 60 -X POST "$CHECKOUT/v1/checkouts" \
  -H 'content-type: application/json' \
  -d '{"orderId":"55555555-5555-4555-8555-555555555555","amount":{"amount":700,"currency":"MXN"}}')
case "$r" in
  *completada*) echo "  OK: sin ninguna foto el sistema sigue correcto, solo reconstruye mas" ;;
  *) echo "  FALLO: sin fotos la compra dio $r"; exit 1 ;;
esac

# --- el outbox, de verdad transaccional ---------------------------------
# La promesa del patron es que el evento y el cambio de estado entran juntos o
# no entra ninguno. Si el `stage` abriera su propia conexion, una transaccion
# revertida dejaria el evento SIN su fila y el relay publicaria algo que nunca
# paso. Nadie lo ve hasta que alguien pregunta por un evento sin su pago.
#
# Esto estaba roto y se midio antes de arreglarlo: 0 pagos y 1 evento.
echo "  una transaccion revertida despues de dejar el evento"
sq() { $COMPOSE exec -T db-payments env PGPASSWORD=local psql -qtAX -U postgres -d payments -c "$1"; }
ENVQ=.env.local
cp "$ENVQ" "$ENVQ.es"
restaurar_env() { mv -f "$ENVQ.es" "$ENVQ" 2>/dev/null || true; }
trap 'restaurar_env' EXIT
printf 'AXON_DEMO_ROMPER_TRAS_STAGE=1\n' >> "$ENVQ"
$COMPOSE stop payments > /dev/null 2>&1
$COMPOSE up -d --wait payments > /dev/null 2>&1

ORDEN=$(sq "SELECT gen_random_uuid()" | tr -d ' \r\n')
codigo=$(curl -sS -o /dev/null -w '%{http_code}' -m 30 -X POST "localhost:${AXON_PORT_payments:-8082}/v1/payments" \
  -H 'content-type: application/json' \
  -d "{\"orderId\":\"$ORDEN\",\"amount\":{\"amount\":500,\"currency\":\"MXN\"}}")
pagos=$(sq "SELECT count(*) FROM payment WHERE order_id = '$ORDEN'" | tr -d ' \r\n')
eventos=$(sq "SELECT count(*) FROM outbox WHERE data->>'orderId' = '$ORDEN'" | tr -d ' \r\n')
restaurar_env
$COMPOSE stop payments > /dev/null 2>&1
$COMPOSE up -d --wait payments > /dev/null 2>&1
if [ "$codigo" = "500" ] && [ "$pagos" -eq 0 ] && [ "$eventos" -eq 0 ]; then
  echo "  OK: 0 pagos y 0 eventos; el evento no sobrevive a la reversion"
  echo "  i con el \`stage\` en su propia conexion esto daba 0 y 1: un cobro"
  echo "    publicado que nunca ocurrio, y nada que lo dijera"
else
  echo "  FALLO: http=$codigo pagos=$pagos eventos=$eventos"
  exit 1
fi
