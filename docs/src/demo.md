# The demo, measured

`examples/` ships three services that really run — `orders`, `payments` and `checkout`,
in TypeScript on Node 24, with no build step — plus one external contract. `./demo.sh`
brings the whole system up and makes **65 checks against reality**: not against a mock,
and not against axon's own asserts.

```sh
cd examples && ./demo.sh    # needs Docker; ~4 minutes from cold
```

It brings up the broker, four Postgres nodes with [pgdog](https://pgdog.dev) in front,
MinIO, Jaeger, flagd, ClickHouse, the edge and the three services — all of it emitted by
`axon infra --target local`, nothing hand-written.

Every line below is printed by a script in `examples/`, and a test checks that: four
consecutive words of each quoted line have to appear in whatever prints it. A page that
quotes a demo which stopped existing is the same drift this project hunts, from the other
side.

## The whole run

```console
$ cd examples && ./demo.sh
==> generating the local infrastructure from the manifests
==> bringing up the broker, the databases, the migrations and the services
==> POST /v1/tenants/{tenantId}/orders
{"orderId":"ea2a86e1-5525-488e-b8b0-10b55c163df4"}
==> waiting for the chain to propagate

==> the real causal chain
└─ POST /v1/tenants/{tenantId}/orders <- http
   └─ order.placed@v1 <- orders
      └─ payment.captured@v1 <- payments

==> the trace in OpenTelemetry
  orders/POST /v1/tenants/{tenantId}/orders
      orders/publish order.placed@v1
          payments/process order.placed@v1
              payments/stage payment.captured@v1
                  payments/publish payment.captured@v1
  OK: 5 spans, one root, no orphans, crossing ['orders', 'payments']
  and the correlationId matches the log's: 5299db21-eaf8-409b-98e8-37809ca67576

==> expected (manifest) vs real (envelope log)
OK: the system does exactly what it declares

==> the registry, from what is RUNNING
  OK: 3 services discovered live, and they declare what the repo says

==> tenant isolation through the pooler
  a query with no tenant in the WHERE
  OK: pgdog rejects it at the router, before touching a node
  20 connections alternating tenant, each also asking for the other one's
  OK: 20 of 20 saw 1 row of their own and 0 of the tenant they asked for
  i pgdog cleans the connection when handing it back: 0 of 20 inherited the value.

==> the saga: compensation and resume, measured
  a checkout below the ceiling
    {"state":"completed"}
  a checkout ABOVE the ceiling: step 2 fails after the charge
    {"state":"compensated"}
  OK: the charge was undone and the merchant was not paid
  a saga stranded in another process, resumed by the sweep
    {"claimed":1,"completed":0,"compensated":1,"stuck":0,"pending":false}
  OK: resumed from the journal, compensated, and the refund reached the charge
  OK: a closed saga is not swept again

==> event sourcing and CQRS, measured
  two concurrent writes at the same version
  OK: one got in and the other was rejected; 1 event left at version 3
  i the rejection is the UNIQUE, not a check in the application
  OK: the view agrees with the stream
  OK: 1041ms of real lag; the unprojected event shows up
  OK: 0ms, inside the declared budget of 3000ms
  OK: the relay came back and published it; nobody had to retry by hand
  OK: snapshot at version 2, a multiple of the declared cadence (2)
  OK: the snapshot says 'compensated' and the projection, which did not use it, says the same
  OK: the rules 0 snapshot lives alongside the current one, which says 900 cents
  OK: it applied 11 events of the stream's 11, and no garbage was left
  OK: every rebuilt row matches the last event of its stream
  OK: the dates came from the stream, not from the hour of the rebuild
    14 reads in the 2s the rebuild took; lowest seen: 4 of 4
  OK: nobody saw a half-built view; 11 events applied in the shadow
  i rebuilding in place, the lowest would have been 0
  OK: the ones from another version are gone, and only the newest of each stream is left
  OK: with no snapshot at all the system stays correct, it just rebuilds more
  OK: 0 payments and 0 events; the event does not survive the rollback
  i with `stage` on its own connection this gave 0 and 1: a charge

==> declared vs occurred retries
  declared in the generated code: payout 2 retries, refund 3
  a payout slower than its timeout: how many times it arrives
    {"state":"compensated"}  (13s)
  OK: 3 calls = 1 + 2 retries, exactly what was declared
  OK: 13000ms inside the 60000ms budget
  OK: 3 calls to the refund (2 failures and the one that got through), and the charge was undone
  i without the 3 declared retries, this saga ended up STUCK
    HTTP 500
  OK: 4 calls, the saga was left STUCK and the response did not hide it
  payments restored with no switches

==> quien puede llamar a que, medido
  declarado en el codigo generado: payoutMerchant exige "payments:write"
    sin el scope: HTTP 403  {"title":"insufficient_scope","detail":"missing: payments:write",...}
  OK: 403 insufficient_scope, y nombra el que falta —un 403 sin razon es un ticket
    con el scope: HTTP 200
  OK: el mismo llamado pasa con lo que el manifiesto exige, y nada mas cambio
    con un scope de lectura: HTTP 403
  OK: un token de lectura no mueve dinero. Con solo `required`, si podria

==> the declared failures, measured
  declared in the generated code: payout 2 retries, retriable "rail_busy"
  a payout over the ceiling: a final failure, how many times it arrives
    {"state":"compensated"}
  OK: 1 call. The declared 2 retries were NOT spent on a failure that cannot end differently
  a saturated rail: a retriable failure, how many times it arrives
    {"state":"compensated"}
  OK: 3 calls = 1 + 2 retries. Same policy, and the declaration is the only difference
  i final 1 call vs retriable 3: that is what the declared errors buy, and it is not documentation
  the code on the wire against the manifest
    HTTP 422  {"type":"about:axon/orders/order_rejected","title":"order_rejected","status":422,...}
  OK: order_rejected with 422, the status and the code the manifest declares

==> two versions of the same endpoint, and the retirement of the old one
  declared in the generated code: deprecation @1788220800, sunset Fri, 31 Dec 2027 00:00:00 GMT
  the v1 of the endpoint: what it answers and what it announces
    HTTP 200  Deprecation: @1788220800  Sunset: Fri, 31 Dec 2027 00:00:00 GMT
    Link: </v2/tenants/{tenantId}/orders/{orderId}>; rel="successor-version"
  OK: it still answers 200 and goes out with the declared Deprecation and Sunset
  OK: the Link points at the v2, so nobody has to guess where to go
  OK: the v2 answers the same plus the customer, and announces nothing: it is the current one

==> las reglas declaradas, evaluadas contra la bodega
  declarado: 2 serie(s) —el disparador y sus guardas— contra las vistas de las metricas
  seis dias: el importe en USD estable y luego -20% y -20%, con el conteo intacto
    proposes  orders.gmv_usd_cayendo
      set `free_shipping` to `over_500` (back to `off` when it lifts)
      `gmv` = 64000 for total.currency = USD held the condition for 2 windows, the 3 before were quiet, and 1 guard held
    axon: 1 of 1 rules propose a change; none was applied
  OK: propone mover el flag declarado a la variante declarada, y volver a off al levantarse
  la misma caida pero con un dia que ya la cumplia tres ventanas antes
    quiet     orders.gmv_usd_cayendo  it already held before: proposed on the way in, not once per window
  OK: no repite. Sin cooldown propondria lo mismo cada ventana, y lo que se repite se ignora
  la misma caida del importe, pero ahora el conteo tambien se cae
    quiet     orders.gmv_usd_cayendo  the guard `orders_by_currency` above 0.9 vs previous does not hold
  OK: la guarda la frena. Con una sola metrica esto seria la ley de Goodhart con un cron
  OK: 25 filas antes y despues; la historia sembrada era del demo y se fue

==> declared vs applied rollout
  declared 10%  measured 10.7%  (32 of 300)
  OK: sticky per tenant, and the percentage applies

==> el lazo cerrado: la regla mueve la palanca
  flagd sirve `free_shipping` = off antes de nada
  OK: la regla dice `apply` y sin la bandera igual solo propone —dos cerrojos, no uno
    applied orders.gmv_usd_cayendo: `free_shipping` off -> over_500
    flagd sirve `free_shipping` = over_500
  OK: la palanca se movio y flagd sirve la variante nueva a quien pregunte
    flagd sirve `free_shipping` = off
  OK: al levantarse la condicion vuelve sola
    auditoria: 2026-09-08 free_shipping off -> over_500
    auditoria: 2026-09-08 free_shipping over_500 -> off
  OK: las dos quedaron escritas con lo que la regla leyo

==> the warehouse: schema, funnel and PII
  OK: 1 events in the warehouse, with the generated schema untouched
  OK: 1 flows, 1 reached the charge (100% conversion)
  i the funnel's business latency: 37ms from the order to the charge
  OK: 1 orders and 25000 cents, the same as counting the table by hand
  OK: 1 bucket(s) and 1 currency; the metric groups by what it declares
  la retencion declarada, aplicada en la tabla
    order.placed@v1 2555 dias  ·  payment.captured@v1 730 dias
  OK: la excepcion por evento manda sobre la del servicio, y esta EN la tabla
  OK: 1 hashed, 0 addresses in plaintext
axon: the warehouse has 0 differences against the manifest
  OK: the missing column is detected, and without it that field would be stored nowhere
  OK: restored whole after breaking it: 1 rows, all with their amount
  OK: 1 rows before and after; the loader is idempotent

==> el ingest de Vector, medido
  OK: 2 replicas subscribed to order.placed.v1, both in the queue group
  one real envelope, published to the broker
  OK: 1 row from 2 replicas; the queue group delivers the event once
  and the PII, which is what nobody was checking
  OK: the same hash as the SQL loader, and no address reaches the warehouse
  OK: trace_id out of the traceparent, and the money in its two columns

==> la cadena real, leida del almacen de trazas
  92 spans leidos de Jaeger, sin tocar el log de envelopes
    read from the Jaeger spans
    1 edges between services  4 declared
      ok orders → payments
      quiet checkout → payments  declared and not seen in this trace
  OK: la arista orders → payments esta en las trazas y esta declarada
      undeclared checkout → orders  nobody declares this dependency, and it happened
  OK: una dependencia que ocurre y nadie declara falla, y el edge no la habria visto

==> el pacto de un consumidor que no usa axon
mobile-app → orders  ·  2 interactions, 0 messages
  GET /v1/tenants/t-1/orders/o-1  →  orders.getOrder  ·  reads orderId, total.amount, total.currency
  mobile-app does not read status of getOrder
ok 2 interactions, 0 messages, 0 errors, 0 warnings

==> y el pacto de un consumidor de un topic
reporting → orders  ·  0 interactions, 1 messages
  order.placed@v1  ·  reads orderId, total.amount, total.currency  (and traceparent, type of the envelope)
  reporting does not read customerId, customerEmail of order.placed@v1
ok 0 interactions, 1 messages, 0 errors, 0 warnings

==> quien llama a que, leido del edge
  un cliente ajeno llamando cinco veces a la version deprecada
    34 requests  7 declared routes
           16    1  GET /v1/tenants/{tenantId}/orders/{orderId} · deprecated · 479d left · use `getOrderV2`
            7    1  POST /v1/tenants/{tenantId}/orders
  OK: el log del edge nombra a quien todavia llama la version que se retira el 2027-12-31
  OK: y no afirma que nadie llama lo que el edge no puede ver
  OK: una ruta que nadie declara sale nombrada, con su conteo

==> el tablero, aprovisionado desde el manifiesto
  declarado: 4 pregunta(s) —una por metrica y por embudo— contra las vistas generadas
  Metabase aprovisionado desde cero, sin tocar la interfaz
  conectado a la bodega con los datos del manifiesto (database 2)
  OK: 4 preguntas creadas, cada una apuntando a la vista que declara el manifiesto
    metabase 334700  ·  clickhouse 334700
  OK: la pregunta contesta lo mismo que la vista; la metrica se define en un solo lugar

==> declared vs measured capacity
info: orders: 96 requests measured at 2.4/s
axon: 0 thresholds breached
    throttled 51%  ·  5xx 0%  ·  respuestas esperadas 100%
  OK: pasado su limite degrada con 429 y no se cae con 500, que es para lo que se declara un limite
```

Three things about the numbers, because they are not decoration:

- **The 1041ms of view lag is deliberate.** It comes from the event the concurrency race
  wrote *straight into the stream*, bypassing the projection. The next check —an
  up-to-date checkout— reports 0ms inside the 3000ms budget. It is measured in both
  directions so a pretty number cannot pass for a working one.
- **The two `i` lines in the past tense** (`this gave 0 and 1`, `the lowest would have
  been 0`) are real defects, already fixed. They stayed in the demo as regression guards.
- **`1 events in the warehouse` on a cold start is right**: the envelope log holds only
  the flow the demo fired, and the orders the other checks seed go through the pooler
  straight into Postgres, not over the bus.

## Run it twice

This is the check with the best track record in the project: **three of the worst bugs it
has had were found by running the demo a second time**, not by any test. The per-view
checkpoint that held one number for the whole view, the saga journal's outputs read back
as `undefined` after a resume, and —most recently— a drift check of my own that quietly
deleted the warehouse's history.

Two consecutive runs over the same volumes, both `exit 0`, 35 checks each:

| | run 1 | run 2 |
| --- | --- | --- |
| warehouse | 13 events, 51100 cents | **14 events, 76100 cents** |
| restored after breaking it | 13 rows, all with their amount | **14 rows, all with their amount** |
| loader, run twice | 13 rows before and after | **14 rows before and after** |
| view rebuild | 65 events applied | **83 events** |
| the shadow's window | 101 reads in 12s, lowest 28 of 28 | **128 reads in 14s, lowest 36 of 36** |
| snapshot prune | 8 snapshots, deleted 1, 7 left | 8 snapshots, deleted 1, 7 left |
| real view lag | 1042ms | 1013ms |

The numbers that **grow** are the point: the second run works on the first one's state, so
every assertion holds over data that already existed. The ones that do **not** grow say
something too — the snapshot prune always leaves 8→7, because a snapshot is a cache and
not a history. And the lowest row count seen during a rebuild rises with the volume and
never drops below what was there: nobody saw a half-built view with 83 events any more
than with 11.

## One capability at a time

Each section is a script you can run on its own once the system is up, which is the
shortest way to show one thing:

| | |
| --- | --- |
| `./check-saga.sh` | compensation, resume from the journal, and a closed saga not swept again |
| `./check-es.sh` | optimistic concurrency, view lag, the relay, snapshots, prune, rebuild with the shadow, and the transactional outbox |
| `./check-retries.sh` | the declared retries, occurring, and what they buy |
| `./check-scopes.sh` | un token válido que no alcanza: 403 nombrando el permiso que falta |
| `./check-errors.sh` | a declared failure: the final one arrives once, the retriable one uses the whole budget |
| `./check-versions.sh` | two versions of the same endpoint, and the headers the retired one really sends |
| `python3 check-apply.py 8016` | el lazo cerrado: la palanca se mueve, flagd la sirve, y vuelve sola |
| `./check-rules.sh` | a rule over a metric: it proposes on the way in, does not repeat, and the guard stops it |
| `./check-spans.sh` | la cadena real leída de Jaeger, y una dependencia que ocurre sin estar declarada |
| `./check-traffic.sh` | who still calls the version being retired, read from the edge's real log |
| `axon pact . --check pacts/mobile-app-orders.json` | a consumer with no manifest, crossed against the declared contract |
| `python3 check-metabase.py 3030` | a Metabase provisioned from the manifest, and the question answering the same as the view |
| `./check-pooler.sh` | tenant isolation through pgdog in transaction mode |
| `./check-warehouse.sh` | schema, funnel, metrics, PII and drift detection |
| `python3 check-flags.py localhost:8016 charge_v2 10` | the rollout, applied and sticky |
| `python3 check-trace.py localhost:16686` | the span tree, with no orphans |
| `python3 check-registry.py <disk.json> <running.json>` | the repo against what is deployed |

And with nothing running at all, the projections take a second:

```sh
axon verify   examples          # the rules, over the example itself
axon graph    examples          # the event topology, as Mermaid
axon classes  examples          # the class diagram
axon er       examples          # entity-relationship, from the migrations
axon seq      order.placed@v1 examples
axon cap      examples          # what the CAP side you picked implies
axon analytics examples --target clickhouse
axon infra    examples --target k8s
```
