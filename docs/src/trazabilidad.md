# Trazabilidad

Todo mensaje viaja en un envelope CloudEvents extendido con la cadena causal:

```json
{ "id": "…", "type": "payment.captured@v1", "source": "payments",
  "traceparent": "00-4bf92f…-00f067…-01",
  "correlationId": "…",   // estable en todo el flujo de negocio
  "causationId":   "…" }  // el mensaje que provocó este
```

El emisor generado recibe el mensaje causante y propaga la cadena. No hay forma de
publicar un evento huérfano sin salirse del framework. Y como el `causationId` ya
está ahí, el debug local no necesita colector ni dashboard:

```console
$ axon trace .axon/local.ndjson
flujo c1
└─ order.placed@v1 <- orders
   └─ payment.captured@v1 <- payments
      └─ receipt.sent@v1 <- billing
```

`axon seq` da el flujo **esperado**; `axon trace --seq` da el **real**. Diferenciarlos
es un test de e2e de una línea.

axon **no trae un SDK de observabilidad ni inventa un formato**. No hace falta: el
`traceparent` del envelope *es* el contexto W3C que propaga OTel, así que un span
creado a partir de un mensaje continúa la misma traza aunque el otro extremo esté en
otro lenguaje.

Lo que axon aporta es lo que sí es suyo — levantar el backend en local y poner las
variables estándar en los cuatro targets, con los atributos de recurso **derivados del
manifiesto**:

```yaml
OTEL_SERVICE_NAME:            payments
OTEL_EXPORTER_OTLP_ENDPOINT:  http://traza:4318    # local; en cloud, una variable
OTEL_RESOURCE_ATTRIBUTES:     service.name=payments,axon.owner=equipo-pagos,axon.tier=0,service.version=1.2.0
OTEL_TRACES_SAMPLER:          parentbased_always_on
```

Las mismas variables suben a `gcp`, `aws` y `k8s`: solo cambia el destino, que es una
variable del IaC para que apunte a Cloud Trace, X-Ray, Datadog o lo que uses. **El
muestreo sale del `tier`** — un servicio tier 0 se traza entero, porque cuando se cae
la traza que falta es justo la que hacía falta. En `local` se traza todo sin importar
el tier: descartar el 90% de las trazas mientras depurás no sirve para nada.

El target `local` levanta el backend (Jaeger, que acepta OTLP directo, así que es un
contenedor y no un colector más un almacén) y el `demo.sh` verifica la forma del árbol
en CI:

```console
==> la traza en OpenTelemetry
  orders/POST /v1/orders
      orders/publish order.placed@v1
          payments/process order.placed@v1
              payments/stage payment.captured@v1
                  payments/publish payment.captured@v1
  OK: 5 spans, un raiz, sin huerfanos, cruzando ['orders', 'payments']
```

Cinco spans, un solo raíz, cero huérfanos, cruzando dos procesos y pasando por el relay
del outbox. Lo que se rompe en cuanto alguien inventa un `traceparent` no es que falte
la traza: es que aparece **partida en fragmentos que cuelgan de padres que nunca
existieron**, y en la UI eso se ve como varias trazas cortas en vez de una.
