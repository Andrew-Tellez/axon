#!/bin/sh
# Does tenant isolation still isolate through pgdog in transaction mode? That is
# the question that decides whether the pooler can be recommended: in transaction
# mode the physical connection is recycled between tenants, so the RLS stops
# depending only on the policy and starts depending on the tenant's value not
# surviving the connection.
#
# There are TWO layers here, and they do not protect against the same thing:
#
#   * `[multi_tenant]` in pgdog rejects every query on a table with a tenant
#     column whose WHERE does not filter by it. It lives in the query's text, so
#     the pooler cannot weaken it.
#   * the RLS lives in the connection's GUC, which is exactly what the pooler
#     recycles. It protects against the opposite case: the query that DOES name a
#     tenant, but the wrong one.
#
# None of this is inferred: it is measured against the real pgdog the compose
# brings up.
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"
COMPOSE="docker compose -f axon.local.yml"
A="aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
B="bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"

# psql runs INSIDE the pooler's container and against the pooler: it is the same
# path the service sees, not a shortcut to the node.
via_pooler() {
  $COMPOSE exec -T pooler-orders \
    env PGPASSWORD=local psql -qtAX -h 127.0.0.1 -p 6432 -U postgres -d orders "$@"
}

# The RLS is not applied from here: it is `sql-policies/orders/R__rls.sql`, a
# generated file like any other, and Flyway ran it on every node before the
# service started. Applying it separately would leave the service running for a
# while with no policy.

# Fixed ids: the compose reuses the volume between runs, and seeding new rows
# every time would make the expected counts depend on how many times it ran.
echo "  seeding one order per tenant, over the right path"
for t in "$A" "$B"; do
  via_pooler -v ON_ERROR_STOP=1 -c "BEGIN;
    SET LOCAL ROLE axon_app;
    SET LOCAL axon.tenant = '$t';
    DELETE FROM \"order\" WHERE tenant_id = '$t';
    COMMIT;" > /dev/null
  via_pooler -v ON_ERROR_STOP=1 -c "BEGIN;
    SET LOCAL ROLE axon_app;
    SET LOCAL axon.tenant = '$t';
    INSERT INTO \"order\" (id, customer_id, total_cents, status, tenant_id)
    VALUES ('$t', gen_random_uuid(), 100, 'placed', '$t')
    ON CONFLICT (id) DO NOTHING;
    COMMIT;" > /dev/null
done

# --- layer 1: the query with no tenant never reaches the node --------------
# Without `[multi_tenant]` this query would run and the RLS would filter it. With
# it, it does not even leave the pooler: a full-table `SELECT` is an error, not a
# result that happened to come back empty.
echo "  a query with no tenant in the WHERE"
if out=$(via_pooler -c 'SELECT count(*) FROM "order"' 2>&1); then
  echo "  REJECTION MISSING: pgdog ran a query with no tenant and returned '$out'"
  exit 1
fi
case "$out" in
  *"multi tenant id"*) echo "  OK: pgdog rejects it at the router, before touching a node" ;;
  *) echo "  it failed for another reason, not the tenant guard: $out"; exit 1 ;;
esac

# --- layer 2: the query that asks for the other tenant ---------------------
# This one pgdog accepts —it names a tenant— and routes it to the node where
# those rows live. That it returns zero is the RLS doing its job through a
# recycled connection, which is the only thing that was in doubt.
echo "  20 connections alternating tenant, each also asking for the other one's"
bad=0
i=0
while [ "$i" -lt 20 ]; do
  i=$((i + 1))
  case $((i % 2)) in 0) mine="$A"; other="$B" ;; *) mine="$B"; other="$A" ;; esac
  seen=$(via_pooler -v ON_ERROR_STOP=1 -c "BEGIN;
    -- both SET LOCALs go together and die together: without the ROLE the policy
    -- does not apply —the superuser owner skips it— and without the tenant there
    -- is no policy to apply
    SET LOCAL ROLE axon_app;
    SET LOCAL axon.tenant = '$mine';
    SELECT count(*) FROM \"order\" WHERE tenant_id = '$mine';
    SELECT count(*) FROM \"order\" WHERE tenant_id = '$other';
    COMMIT;" 2>&1 | tr -d ' ' | tr '\n' ',')
  # own=1, other's=0. Anything else —an error included— does not pass.
  [ "$seen" = "1,0," ] || { bad=$((bad + 1)); echo "    connection $i: '$seen'"; }
done
if [ "$bad" -eq 0 ]; then
  echo "  OK: 20 of 20 saw 1 row of their own and 0 of the tenant they asked for"
else
  echo "  FAILED: $bad of 20 connections did not isolate"
  exit 1
fi

# --- the evidence for why the manifest's rule exists -----------------------
# One loose session `SET`, and then clean clients asking which tenant they have
# set. It measures whether pgdog cleans the connection when handing it back to
# the pool, or whether the value survives the client that set it.
echo "  one loose session \`SET\`, and 20 clean clients afterwards"
via_pooler -c "SET axon.tenant = '$A'" > /dev/null
survivors=0
i=0
while [ "$i" -lt 20 ]; do
  i=$((i + 1))
  v=$(via_pooler -c "SELECT coalesce(current_setting('axon.tenant', true), '')" | tr -d ' \n')
  [ -z "$v" ] || survivors=$((survivors + 1))
done
if [ "$survivors" -eq 0 ]; then
  echo "  i pgdog cleans the connection when handing it back: 0 of 20 inherited the value."
  echo "    The \`tenant_binding\` rule does not change: isolation should not depend"
  echo "    on the pooler cleaning up, and that cleanup is declared nowhere in the"
  echo "    manifest."
else
  echo "  ! $survivors of 20 clients inherited the previous tenant."
  echo "    With the RLS in place that is serving somebody else's rows with no error"
  echo "    at all: this is why \`axon verify\` requires \`tenant_binding = \"set_local\"\`."
fi
