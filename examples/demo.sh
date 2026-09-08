#!/bin/sh
# End-to-end demo: brings the system up, fires one flow and checks that the real
# causal chain matches the one the manifest declares.
#
#   cd examples && ./demo.sh
set -eu
cd "$(dirname "$0")"
AXON="${AXON:-../target/release/axon}"

# axon derives the default ports from the service's NAME, so adding one does not
# move another one's port. The demo pins them on purpose: that way the script and
# the compose talk about the same number without reading each other.
export AXON_PORT_orders="${AXON_PORT_orders:-8080}"
export AXON_PORT_checkout="${AXON_PORT_checkout:-8081}"
export AXON_PORT_payments="${AXON_PORT_payments:-8082}"
PORT="$AXON_PORT_orders"

# When this fails in CI there is nobody watching the screen: the diagnosis has to
# stay in the run's log, or it is lost.
STEP="start"
step() {
  STEP="$1"
  echo "==> $1"
}

diagnose() {
  code=$?
  [ "$code" -eq 0 ] && return 0
  echo
  echo "==> FAILED at step '$STEP' (code $code)"
  # Actions cuts off at 10 annotations per step, so the essentials go first and in
  # a single one: without this the failure's summary is lost among the logs.
  if [ -n "${GITHUB_ACTIONS:-}" ]; then
    state=$(docker compose -f axon.local.yml ps -a --format '{{.Service}}={{.State}}' 2>/dev/null | tr '\n' ' ')
    echo "::error title=failure::step='$STEP' code=$code containers: $state"
  fi
  docker compose -f axon.local.yml ps -a || true
  for s in $(docker compose -f axon.local.yml config --services 2>/dev/null); do
    echo
    echo "--- logs of $s (last 40) ---"
    docker compose -f axon.local.yml logs --tail 40 "$s" 2>&1 || true
    # In Actions the run's log is not public but the annotations are: the last
    # lines of each service come out as ::error:: too.
    # Only from the ones that did not end well: the rest would fill the annotation quota
    if [ -n "${GITHUB_ACTIONS:-}" ]; then
      case "$(docker compose -f axon.local.yml ps -a --format '{{.State}}' --status running --status exited "$s" 2>/dev/null)" in
        running|exited) : ;;
        *) docker compose -f axon.local.yml logs --tail 8 --no-log-prefix "$s" 2>&1 \
             | tr '\n' '|' | sed "s/^/::error title=$s::/" || true ;;
      esac
    fi
  done
  echo
  echo "--- recorded envelopes ---"
  cat .axon/log/local.ndjson 2>/dev/null || echo "(none)"
  return "$code"
}
trap diagnose EXIT

step "generating the local infrastructure from the manifests"
"$AXON" infra . --target local > axon.local.yml

# The envelope log lives in its own subdirectory because ClickHouse `chown`s
# whatever is mounted at its user_files: with `.axon` itself there, on Linux every
# file written afterwards fails with `Permission denied`.
mkdir -p .axon/log
# flagd reads this JSON: the flags come out of the manifest too
"$AXON" flags . > .axon/flags.json

# pgdog reads two files, and both come out of the manifest. The hosts it names
# under `--target local` are the containers `axon infra` just emitted.
mkdir -p .axon/pgdog/orders
"$AXON" pooler . --service orders --target local > .axon/pgdog/orders/pgdog.toml
"$AXON" pooler . --service orders --target local --users > .axon/pgdog/orders/users.toml

step "bringing up the broker, the databases, the migrations and the services"
# `--remove-orphans`: a service that changes name leaves the old container
# running, and that one keeps holding its port. The new compose comes up just the
# same and fails to publish the port, which reads as a port problem and not as
# what it is.
docker compose -f axon.local.yml up -d --build --wait --remove-orphans

rm -f .axon/log/local.ndjson
mkdir -p .axon

TENANT="${AXON_TENANT:-11111111-1111-4111-8111-111111111111}"
step "POST /v1/tenants/{tenantId}/orders"
if ! response=$(curl -sS --fail-with-body --max-time 30 \
    -X POST "localhost:$PORT/v1/tenants/$TENANT/orders" \
    -H 'content-type: application/json' \
    -d '{"customerId":"11111111-1111-4111-8111-111111111111","total":{"amount":25000,"currency":"MXN"}}' 2>&1); then
  code=$?
  echo "curl exited $code: $response"
  [ -n "${GITHUB_ACTIONS:-}" ] && echo "::error title=POST::curl=$code response=$response"
  exit 1
fi
echo "$response"

step "waiting for the chain to propagate"
i=0
while [ "$(wc -l < .axon/log/local.ndjson 2>/dev/null || echo 0)" -lt 3 ]; do
  i=$((i + 1))
  [ "$i" -gt 45 ] && { echo "the chain did not complete"; exit 1; }
  sleep 1
done

echo
step "the real causal chain"
"$AXON" trace .axon/log/local.ndjson
echo
step "the trace in OpenTelemetry"
UI="localhost:${AXON_TRACE_UI_PORT:-16686}"
i=0
until curl -fsS "http://$UI/api/services" 2>/dev/null | grep -q payments; do
  i=$((i + 1))
  [ "$i" -gt 45 ] && { echo "no trace reached the collector"; exit 1; }
  sleep 1
done
python3 check-trace.py "$UI"

step "expected (manifest) vs real (envelope log)"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# It is narrowed to the flow this demo fired. Comparing one expected flow against
# ALL the ones in the log only works if there is exactly one, and that stops being
# true as soon as anything else touches the system —a load test, for instance.
FLOW=$(python3 -c 'import json;print(json.loads(open(".axon/log/local.ndjson").readline())["correlationId"])')
"$AXON" seq order.placed@v1 . --events > "$tmp/expected"
"$AXON" trace .axon/log/local.ndjson --seq --correlation "$FLOW" > "$tmp/real"
if diff -u "$tmp/expected" "$tmp/real"; then
  echo "OK: the system does exactly what it declares"
else
  echo "DRIFT: the system does not do what it declares"
  exit 1
fi


step "the registry, from what is RUNNING"
# Each service serves its own manifest at the path the generated contract names,
# so a registry can be built from what is deployed instead of from what somebody
# remembered to commit. What is checked is that both agree: if a service were
# running a manifest other than the one in the repo, this is where it shows.
"$AXON" discover . > "$tmp/on-disk.json"
"$AXON" discover \
  "http://localhost:$AXON_PORT_orders" \
  "http://localhost:$AXON_PORT_payments" \
  "http://localhost:$AXON_PORT_checkout" > "$tmp/running.json"
# `source` differs on purpose —a file path against a URL— and `stripe` is only on
# disk, because an external contract is frozen and serves nothing.
python3 check-registry.py "$tmp/on-disk.json" "$tmp/running.json"

step "tenant isolation through the pooler"
./check-pooler.sh

step "the saga: compensation and resume, measured"
./check-saga.sh

step "event sourcing and CQRS, measured"
./check-es.sh

step "declared vs occurred retries"
./check-retries.sh

step "quien puede llamar a que, medido"
./check-scopes.sh

step "the declared failures, measured"
./check-errors.sh

step "two versions of the same endpoint, and the retirement of the old one"
./check-versions.sh

step "declared vs applied rollout"
python3 check-flags.py "localhost:${AXON_FLAGS_PORT:-8016}" charge_v2 10

# Va ANTES del chequeo de bodega: siembra historia y la borra al terminar, y su
# limpieza inicial tambien arregla lo que dejo una corrida interrumpida. Al
# reves, el embudo contaria esas filas sembradas como flujos que nunca cobraron.
step "las reglas declaradas, evaluadas contra la bodega"
./check-rules.sh

step "el lazo cerrado: la regla mueve la palanca"
python3 check-apply.py "localhost:${AXON_FLAGS_PORT:-8016}"

step "the warehouse: schema, funnel and PII"
./check-warehouse.sh

step "la cadena real, leida del almacen de trazas"
./check-spans.sh

step "el pacto de un consumidor que no usa axon"
"$AXON" pact . --check pacts/mobile-app-orders.json

# La otra mitad de la superficie: un topic. Lo que alguien lee de un mensaje no
# se observa desde aqui —el edge ve una llamada, a un consumidor de un topic no
# lo ve nadie— asi que o lo dicen, o no se sabe.
step "y el pacto de un consumidor de un topic"
"$AXON" pact . --check pacts/reporting-orders.json

step "quien llama a que, leido del edge"
./check-traffic.sh

step "el tablero, aprovisionado desde el manifiesto"
python3 check-metabase.py "${AXON_BI_PORT:-3030}"

step "declared vs measured capacity"
if command -v k6 >/dev/null 2>&1; then
  "$AXON" load orders.toml > .axon/load.js
  # Contra el EDGE y no contra el servicio: el `rate_limit` declarado lo aplica
  # el edge, y pegarle directo al servicio mediria un limite que nadie impone.
  # La rampa sube hasta el declarado y un 25% por encima, que es donde se ve si
  # degrada o se cae.
  k6 run --quiet --summary-export=.axon/load.json \
    --env AXON_BASE="http://localhost:${AXON_EDGE_PORT:-8000}" \
    --env AXON_LOAD_DURATION="${AXON_LOAD_DURATION:-40}" \
    .axon/load.js > /dev/null 2>&1 || true
  "$AXON" load orders.toml --check .axon/load.json
  # Y lo que la rampa existe para distinguir: 429 es el limite declarado
  # funcionando, 5xx es el servicio rompiendose.
  python3 check-limit.py .axon/load.json
else
  echo "  skipped: k6 is not installed"
fi
