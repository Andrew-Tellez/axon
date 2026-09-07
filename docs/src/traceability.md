# Traceability

Every message travels in a CloudEvents envelope extended with the causal chain:

```json
{ "id": "…", "type": "payment.captured@v1", "source": "payments",
  "traceparent": "00-4bf92f…-00f067…-01",
  "correlationId": "…",   // stable across the whole business flow
  "causationId":   "…" }  // the message that caused this one
```

The generated emitter takes the causing message and propagates the chain. There is no
way to publish an orphan event without stepping outside the framework. And since the
`causationId` is already there, local debugging needs no collector and no dashboard:

```console
$ axon trace .axon/local.ndjson
flow c1
└─ order.placed@v1 <- orders
   └─ payment.captured@v1 <- payments
      └─ receipt.sent@v1 <- billing
```

`axon seq` gives the **expected** flow; `axon trace --seq` gives the **real** one.
Diffing them is a one-line end-to-end test.

axon **ships no observability SDK and invents no format**. It does not need to: the
envelope's `traceparent` *is* the W3C context OTel propagates, so a span created from a
message continues the same trace even when the other end is in another language.

What axon adds is what is actually its own — bringing the backend up locally and setting
the standard variables on all four targets, with the resource attributes **derived from
the manifest**:

```yaml
OTEL_SERVICE_NAME:            payments
OTEL_EXPORTER_OTLP_ENDPOINT:  http://trace:4318    # local; in the cloud, a variable
OTEL_RESOURCE_ATTRIBUTES:     service.name=payments,axon.owner=payments-team,axon.tier=0,service.version=1.2.0
OTEL_TRACES_SAMPLER:          parentbased_always_on
```

The same variables go up on `gcp`, `aws` and `k8s`: only the destination changes, and it
is an IaC variable so it can point at Cloud Trace, X-Ray, Datadog or whatever you use.
**The sampling comes from the `tier`** — a tier 0 service is traced whole, because when
it goes down the missing trace is exactly the one that was needed. On `local` everything
is traced regardless of the tier: dropping 90% of the traces while you debug is no use
at all.

The `local` target brings the backend up (Jaeger, which accepts OTLP directly, so it is
one container and not a collector plus a store) and `demo.sh` verifies the tree's shape
in CI:

```console
==> the trace in OpenTelemetry
  orders/POST /v1/tenants/{tenantId}/orders
      orders/publish order.placed@v1
          payments/process order.placed@v1
              payments/stage payment.captured@v1
                  payments/publish payment.captured@v1
  OK: 5 spans, one root, no orphans, crossing ['orders', 'payments']
```

Five spans, a single root, zero orphans, crossing two processes and going through the
outbox relay. What breaks the moment somebody invents a `traceparent` is not that the
trace is missing: it is that it shows up **split into fragments hanging off parents that
never existed**, and in the UI that looks like several short traces instead of one.
