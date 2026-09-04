#!/bin/sh
# ¿Sigue aislando el aislamiento por inquilino a traves de pgdog en modo
# transaccion? Es la pregunta que decide si el pooler se puede recomendar: en
# modo transaccion la conexion fisica se recicla entre inquilinos, asi que la
# RLS deja de depender solo de la politica y pasa a depender de que el valor
# del inquilino no sobreviva a la conexion.
#
# Aca hay DOS capas, y no protegen de lo mismo:
#
#   * `[multi_tenant]` en pgdog rechaza toda consulta sobre una tabla con
#     columna de inquilino cuyo WHERE no filtre por ella. Vive en el texto de
#     la consulta, asi que el pooler no la puede debilitar.
#   * la RLS vive en la GUC de la conexion, que es justo lo que el pooler
#     recicla. Protege del caso contrario: la consulta que SI nombra un
#     inquilino, pero el ajeno.
#
# Nada de esto se infiere: se mide contra el pgdog real que levanta el compose.
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
COMPOSE="docker compose -f axon.local.yml"
A="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
B="bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"

# psql corre DENTRO del contenedor del pooler y contra el pooler: es el mismo
# camino que ve el servicio, no un atajo al nodo.
por_pooler() {
  $COMPOSE exec -T pooler-orders \
    env PGPASSWORD=local psql -qtAX -h 127.0.0.1 -p 6432 -U postgres -d orders "$@"
}

# La RLS no se aplica desde aca: es `sql/orders/090_rls.expand.sql`, una
# migracion generada como cualquier otra, y Flyway la corrio en cada nodo antes
# de que el servicio arrancara. Aplicarla aparte dejaria al servicio corriendo
# un rato sin politica.

# Ids fijos: el compose reusa el volumen entre corridas, y sembrar filas nuevas
# cada vez haria que los conteos esperados dependieran de cuantas veces se corrio.
echo "  sembrando una orden por inquilino, por el camino correcto"
for t in "$A" "$B"; do
  por_pooler -v ON_ERROR_STOP=1 -c "BEGIN;
    SET LOCAL ROLE axon_app;
    SET LOCAL axon.tenant = '$t';
    DELETE FROM \"order\" WHERE tenant_id = '$t';
    COMMIT;" > /dev/null
  por_pooler -v ON_ERROR_STOP=1 -c "BEGIN;
    SET LOCAL ROLE axon_app;
    SET LOCAL axon.tenant = '$t';
    INSERT INTO \"order\" (id, customer_id, total_cents, status, tenant_id)
    VALUES ('$t', gen_random_uuid(), 100, 'nueva', '$t')
    ON CONFLICT (id) DO NOTHING;
    COMMIT;" > /dev/null
done

# --- capa 1: la consulta sin inquilino no llega al nodo --------------------
# Sin `[multi_tenant]` esta consulta se ejecutaria y la RLS la filtraria. Con
# el, ni sale del pooler: un `SELECT` de tabla completa es un error, no un
# resultado que resulto vacio por suerte.
echo "  una consulta sin inquilino en el WHERE"
if salida=$(por_pooler -c 'SELECT count(*) FROM "order"' 2>&1); then
  echo "  RECHAZO AUSENTE: pgdog ejecuto una consulta sin inquilino y devolvio '$salida'"
  exit 1
fi
case "$salida" in
  *"multi tenant id"*) echo "  OK: pgdog la rechaza en el router, antes de tocar un nodo" ;;
  *) echo "  fallo por otro motivo, no por el guardia de inquilino: $salida"; exit 1 ;;
esac

# --- capa 2: la consulta que pide el inquilino ajeno ------------------------
# Esta pgdog la acepta —nombra un inquilino— y la rutea al nodo donde viven
# esas filas. Que devuelva cero es la RLS haciendo su trabajo a traves de una
# conexion reciclada, que es lo unico que estaba en duda.
echo "  20 conexiones alternando inquilino, cada una pidiendo tambien el ajeno"
malas=0
i=0
while [ "$i" -lt 20 ]; do
  i=$((i + 1))
  case $((i % 2)) in 0) yo="$A"; otro="$B" ;; *) yo="$B"; otro="$A" ;; esac
  vistas=$(por_pooler -v ON_ERROR_STOP=1 -c "BEGIN;
    -- los dos SET LOCAL van juntos y mueren juntos: sin el ROLE la politica
    -- no aplica —el dueno superusuario se la salta— y sin el tenant no hay
    -- politica que aplicar
    SET LOCAL ROLE axon_app;
    SET LOCAL axon.tenant = '$yo';
    SELECT count(*) FROM \"order\" WHERE tenant_id = '$yo';
    SELECT count(*) FROM \"order\" WHERE tenant_id = '$otro';
    COMMIT;" 2>&1 | tr -d ' ' | tr '\n' ',')
  # propias=1, ajenas=0. Cualquier otra cosa —incluido un error— no pasa.
  [ "$vistas" = "1,0," ] || { malas=$((malas + 1)); echo "    conexion $i: '$vistas'"; }
done
if [ "$malas" -eq 0 ]; then
  echo "  OK: 20 de 20 vieron 1 fila propia y 0 del inquilino que pidieron ajeno"
else
  echo "  FALLO: $malas de 20 conexiones no aislaron"
  exit 1
fi

# --- la evidencia de por que existe la regla del manifiesto ----------------
# Un solo `SET` de sesion suelto, y despues clientes limpios preguntando que
# inquilino tienen puesto. Mide si pgdog limpia la conexion al devolverla al
# pool, o si el valor sobrevive al cliente que lo puso.
echo "  un \`SET\` de sesion suelto, y 20 clientes limpios despues"
por_pooler -c "SET axon.tenant = '$A'" > /dev/null
sobreviven=0
i=0
while [ "$i" -lt 20 ]; do
  i=$((i + 1))
  v=$(por_pooler -c "SELECT coalesce(current_setting('axon.tenant', true), '')" | tr -d ' \n')
  [ -z "$v" ] || sobreviven=$((sobreviven + 1))
done
if [ "$sobreviven" -eq 0 ]; then
  echo "  i pgdog limpia la conexion al devolverla: 0 de 20 heredaron el valor."
  echo "    La regla de \`tenant_binding\` no cambia: el aislamiento no deberia"
  echo "    depender de que el pooler limpie, y esa limpieza no esta declarada"
  echo "    en ninguna parte del manifiesto."
else
  echo "  ! $sobreviven de 20 clientes heredaron el inquilino del anterior."
  echo "    Con la RLS puesta eso es servir las filas de otro sin un solo error:"
  echo "    por esto \`axon verify\` exige \`tenant_binding = \"set_local\"\`."
fi
