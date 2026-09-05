#!/bin/sh
# La bodega, medida: que el esquema generado acepte los eventos reales y que el
# embudo declarado cuente el flujo que de verdad ocurrio.
#
# Esto es lo que faltaba: el esquema se generaba para tres dialectos y solo GCP
# tenia camino de ingesta, asi que se podia aplicar en las otras y quedarse con
# tablas vacias sin que nada avisara.
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
COMPOSE="docker compose -f axon.local.yml"

ch() { $COMPOSE exec -T bodega clickhouse-client --user local --password local "$@"; }

echo "  aplicando el esquema generado"
# El esquema sale con `@dataset` como parametro; en local la base es `axon`.
"$AXON" analytics . --target clickhouse | sed 's/"@dataset\.\([a-z0-9_]*\)"/axon.\1/g' > .axon/bodega.sql
ch --multiquery < .axon/bodega.sql

echo "  cargando el log de envelopes"
# El log lo escribe el propio target local. La ruta es la que ve ClickHouse.
"$AXON" analytics . --cargar local.ndjson --dataset axon > .axon/cargar.sql
ch --param_salt=demo-salt --multiquery < .axon/cargar.sql

filas=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
[ "$filas" -ge 1 ] || { echo "  FALLO: el esquema se aplico y la bodega quedo vacia"; exit 1; }
echo "  OK: $filas eventos en la bodega, con el esquema generado sin tocarlo"

# --- el embudo cuenta el flujo real ---------------------------------------
# La cadena causal declarada dice que un `order.placed@v1` lleva a un
# `payment.captured@v1`. El embudo sale de esa declaracion, asi que si el
# sistema hace lo que declara, la conversion es 1.
echo "  el embudo declarado contra el flujo real"
# Solo los flujos que EMPEZARON: la vista agrupa por correlationId sobre la
# union de las dos tablas, asi que sin este filtro se cuentan tambien flujos
# que solo tienen el paso 2 —de una corrida anterior, o de un cobro que no
# nacio de una orden— y eso ya no es la conversion de este embudo.
embudo=$(ch -q "SELECT count(*), countIf(paso_2_payment_captured_v1 IS NOT NULL)
                  FROM axon.embudo_order_placed_v1
                 WHERE paso_1_order_placed_v1 IS NOT NULL FORMAT TSV")
flujos=$(printf '%s' "$embudo" | cut -f1)
convertidos=$(printf '%s' "$embudo" | cut -f2)
if [ "$flujos" -ge 1 ] && [ "$flujos" = "$convertidos" ]; then
  echo "  OK: $flujos flujos, $convertidos llegaron al cobro (conversion 100%)"
else
  echo "  FALLO: $flujos flujos y solo $convertidos convertidos"
  exit 1
fi

# y la latencia de negocio es un numero, no una promesa
ms=$(ch -q "SELECT round(avg(ms_hasta_payment_captured_v1)) FROM axon.embudo_order_placed_v1
             WHERE ms_hasta_payment_captured_v1 IS NOT NULL")
# y coincide con lo que hay en la tabla del paso 1: si no, el embudo esta
# contando flujos de otra parte
[ "$flujos" = "$filas" ] || { echo "  FALLO: $flujos flujos en el embudo y $filas ordenes en la tabla"; exit 1; }
echo "  i latencia de negocio del embudo: ${ms}ms de la orden al cobro"

# --- el dato personal no viaja en claro ----------------------------------
# `pii = "hash"` en el manifiesto. Que la columna sea un hash y no el correo
# es la unica forma de saber que la politica se aplico.
echo "  el campo personal, como lo declara el manifiesto"
crudo=$(ch -q "SELECT count(*) FROM axon.order_placed_v1 WHERE customer_email_hash LIKE '%@%'")
hashes=$(ch -q "SELECT count(*) FROM axon.order_placed_v1
                 WHERE length(customer_email_hash) = 64 AND customer_email_hash NOT LIKE '%@%'")
if [ "$crudo" -eq 0 ] && [ "$hashes" -ge 1 ]; then
  echo "  OK: $hashes hasheados, 0 correos en claro"
else
  echo "  FALLO: crudos=$crudo hasheados=$hashes"
  exit 1
fi

# --- cargar dos veces no duplica -----------------------------------------
# Un cargador periodico corre muchas veces sobre el mismo log. Si no filtra por
# lo ya cargado, cada evento se multiplica y el embudo miente sin fallar.
echo "  el cargador, corrido dos veces"
antes=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
ch --param_salt=demo-salt --multiquery < .axon/cargar.sql
despues=$(ch -q "SELECT count(*) FROM axon.order_placed_v1")
if [ "$antes" = "$despues" ]; then
  echo "  OK: $antes filas antes y despues; el cargador es idempotente"
else
  echo "  FALLO: $antes -> $despues, el cargador duplica"
  exit 1
fi
