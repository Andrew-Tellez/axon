#!/bin/sh
# Las reglas declaradas, evaluadas contra la bodega real.
#
# Lo declarado sale del MANIFIESTO —axon emite el SQL y no tiene credenciales—.
# Lo medido son las filas que contesta ClickHouse. Y la decision —"dos ventanas
# seguidas, tranquilo antes, y cada guarda aguantando en esas mismas ventanas"—
# la toma el compilador sobre esas filas, que es donde se puede probar sin una
# bodega.
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
COMPOSE="docker compose -f axon.local.yml"
ch() { $COMPOSE exec -T warehouse clickhouse-client --user local --password local "$@"; }

# El esquema, por si este chequeo corre solo: las vistas de las metricas son
# CREATE OR REPLACE, asi que aplicarlas otra vez no cuesta nada y evita que la
# regla lea una vista que todavia no existe.
"$AXON" analytics . --target clickhouse | sed 's/"@dataset\.\([a-z0-9_]*\)"/axon.\1/g' > .axon/warehouse.sql
ch --multiquery < .axon/warehouse.sql

"$AXON" rules . | sed 's/"@dataset\.\([a-z0-9_]*\)"/axon.\1/g' > .axon/rules.sql
echo "  declarado: $(grep -c '^SELECT' .axon/rules.sql) serie(s) —el disparador y sus guardas— contra las vistas de las metricas"

# Dos filas por dia SIEMPRE: el conteo de ordenes se mantiene y lo que cambia
# es el importe. Es justo lo que la guarda distingue —no se fue la demanda, se
# encogio el ticket— y es lo que el envio gratis puede mover.
seed() {
  ch -q "INSERT INTO axon.order_placed_v1
         (event_id, event_type, source, event_time, correlation_id, order_id, total_amount, total_currency)
         VALUES (generateUUIDv4(), 'order.placed@v1', 'rules-demo',
                 now() - INTERVAL $1 DAY, generateUUIDv4(), generateUUIDv4(), $2, 'USD'),
                (generateUUIDv4(), 'order.placed@v1', 'rules-demo',
                 now() - INTERVAL $1 DAY, generateUUIDv4(), generateUUIDv4(), $2, 'USD')"
}
wipe() {
  ch -q "ALTER TABLE axon.order_placed_v1 DELETE WHERE source = 'rules-demo' SETTINGS mutations_sync = 2"
}
evaluate() {
  ch --multiquery --format TSV < .axon/rules.sql > .axon/rules.tsv
  "$AXON" rules . --check .axon/rules.tsv
}

# --- entra en la condicion: propone -------------------------------------
# Se limpia ANTES tambien: una corrida anterior interrumpida deja historia
# sembrada, y la regla la leeria como si fuera del sistema.
wipe
# El conteo se toma DESPUES de limpiar: si una corrida anterior dejo filas, el
# antes seria mas alto que el despues y el chequeo del final acusaria al que
# limpio en vez de al que interrumpio.
before=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
echo "  seis dias: el importe en USD estable y luego -20% y -20%, con el conteo intacto"
for d in 6 5 4 3; do seed "$d" 50000; done
seed 2 40000
seed 1 32000
out=$(evaluate)
echo "$out" | sed 's/^/    /'
case "$out" in
  *proposes*free_shipping*over_500*) : ;;
  *) echo "  FALLO: no propuso lo que la regla declara"; exit 1 ;;
esac
case "$out" in
  *"1 of 1 rules propose"*)
    echo "  OK: propone mover el flag declarado a la variante declarada, y volver a off al levantarse" ;;
  *) echo "  FALLO: el conteo de propuestas no es 1"; exit 1 ;;
esac

# --- ya venia cumpliendose: no repite -----------------------------------
echo "  la misma caida pero con un dia que ya la cumplia tres ventanas antes"
wipe
seed 6 50000
for d in 5 4 3; do seed "$d" 40000; done
seed 2 32000
seed 1 25500
out=$(evaluate)
echo "$out" | sed 's/^/    /'
case "$out" in
  *quiet*"already held"*)
    echo "  OK: no repite. Sin cooldown propondria lo mismo cada ventana, y lo que se repite se ignora" ;;
  *) echo "  FALLO: propuso dos veces la misma cosa"; exit 1 ;;
esac

# --- y la guarda: si tambien cae la demanda, se calla --------------------
# Misma caida del importe, pero ahora el conteo tambien se cae. El envio gratis
# no es la palanca de eso, y la regla lo dice en vez de proponerlo igual.
echo "  la misma caida del importe, pero ahora el conteo tambien se cae"
wipe
for d in 6 5 4 3; do seed "$d" 50000; done
ch -q "INSERT INTO axon.order_placed_v1
       (event_id, event_type, source, event_time, correlation_id, order_id, total_amount, total_currency)
       VALUES (generateUUIDv4(), 'order.placed@v1', 'rules-demo', now() - INTERVAL 2 DAY,
               generateUUIDv4(), generateUUIDv4(), 80000, 'USD')"
ch -q "INSERT INTO axon.order_placed_v1
       (event_id, event_type, source, event_time, correlation_id, order_id, total_amount, total_currency)
       VALUES (generateUUIDv4(), 'order.placed@v1', 'rules-demo', now() - INTERVAL 1 DAY,
               generateUUIDv4(), generateUUIDv4(), 64000, 'USD')"
out=$(evaluate)
echo "$out" | sed 's/^/    /'
case "$out" in
  *quiet*guard*"does not hold"*)
    echo "  OK: la guarda la frena. Con una sola metrica esto seria la ley de Goodhart con un cron" ;;
  *) echo "  FALLO: propuso con la guarda caida"; exit 1 ;;
esac

# La historia sembrada se va: es del demo, no del sistema.
wipe
after=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
if [ "$before" = "$after" ]; then
  echo "  OK: $after filas antes y despues; la historia sembrada era del demo y se fue"
else
  echo "  FALLO: quedaron filas sembradas ($before antes, $after despues)"
  exit 1
fi
