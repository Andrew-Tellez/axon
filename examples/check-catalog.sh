#!/bin/sh
# El catalogo, medido. Lo que se declara es UNA lista: la tabla, la semilla y el
# tipo union salen del mismo sitio. Lo que se comprueba aqui es que siguen
# siendo la misma el dia que alguien la cambia.
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
COMPOSE="docker compose -f axon.local.yml"

# El primer nodo del sharder: el catalogo se siembra en todos, y con mirar uno
# basta para saber si el job corrio.
ch() { $COMPOSE exec -T db-orders-0 psql -U postgres -d orders -tAc "$1"; }

rows=$(ch "SELECT count(*) FROM catalog_currency")
[ "$rows" = "3" ] || { echo "  FALLO: $rows filas y el manifiesto declara 3"; exit 1; }
echo "  OK: las 3 monedas declaradas estan en la tabla, sembradas por el job que el target emite"

# Lo que de verdad mantiene las dos listas iguales: quitar un valor del
# manifiesto lo quita de la tabla. Sin el DELETE, el codigo deja de ofrecerlo y
# la base de datos lo sigue aceptando, y nadie ve la diferencia.
echo "  una moneda retirada del manifiesto"
sed 's/^  { code = "CLP".*$//' orders.toml > .axon/orders-sin-clp.toml
mkdir -p .axon/sin-clp && cp .axon/orders-sin-clp.toml .axon/sin-clp/orders.toml
cp payments.toml checkout.toml stripe.external.toml axon.policy.toml .axon/sin-clp/ 2>/dev/null || true
cp -r payments .axon/sin-clp/ 2>/dev/null || true
"$AXON" catalog .axon/sin-clp --service orders > sql-catalog/orders/R__catalog.sql
$COMPOSE up -d --wait catalog-db-orders-0 >/dev/null 2>&1 || true
$COMPOSE restart catalog-db-orders-0 >/dev/null 2>&1 || true
sleep 3
after=$(ch "SELECT count(*) FROM catalog_currency")
# se deja como estaba, pase lo que pase
"$AXON" catalog . --service orders > sql-catalog/orders/R__catalog.sql
if [ "$after" != "2" ]; then
  echo "  FALLO: quedan $after monedas y el manifiesto ya solo declara 2"
  exit 1
fi
echo "  OK: el valor retirado desaparece de la tabla; sin eso el codigo deja de ofrecerlo y la base lo sigue aceptando"
