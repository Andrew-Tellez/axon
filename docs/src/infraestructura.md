# Infraestructura

El manifiesto no menciona ningún proveedor. `axon infra` produce primero un **plan
neutral** y después lo renderiza:

| Target | Edge | Mensajería | Cómputo | Estado, objetos y secretos |
| --- | --- | --- | --- | --- |
| `local` | Traefik | NATS JetStream | tus servicios con `build:` | Postgres + MinIO + Jaeger + migraciones aplicadas |
| `gcp` | url_map + backends | Pub/Sub con push y DLQ | Cloud Run + service account | Cloud SQL, GCS + Cloud CDN, Secret Manager |
| `aws` | API Gateway v2 | SNS → SQS con redrive | ECS Fargate + autoscaling | RDS, S3 + CloudFront, Secrets Manager |
| `k8s` | Gateway API HTTPRoute | Knative Broker/Trigger | Deployment + Service + HPA | External Secrets |
| `plan` | — | — | — | El plan neutral en JSON, para renderizarlo vos mismo |

Cada target despliega el sistema completo: la suscripción entrega a un workload que
existe, y el secreto llega a la variable de entorno del contenedor. La imagen es lo
único que no se declara — cambia en cada deploy, así que sale como variable del IaC.

**Local es un target más, no un subsistema aparte.** Por eso local y producción no
pueden divergir: salen de la misma declaración.

```sh
axon infra manifests/ --target local > axon.local.yml
docker compose -f axon.local.yml up -d --wait   # broker + postgres + migraciones + tus servicios
```

## La demo, en dos comandos

`examples/` trae dos servicios que corren de verdad — `orders` y `payments`, en
TypeScript sobre Node 24, sin paso de build. `./demo.sh` levanta el sistema
completo, dispara un flujo y comprueba que la realidad coincide con lo declarado:

```console
$ cd examples && ./demo.sh
==> POST /v1/orders
{"orderId":"64f37016-7ed5-4be5-b45c-bd0074e9df2c"}

==> cadena causal real
flujo fce36b59-36c7-4d88-81d1-25d90988e204
└─ POST /v1/orders <- http
   └─ order.placed@v1 <- orders
      └─ payment.captured@v1 <- payments

==> esperado (manifiesto) vs real (log de envelopes)
OK: el sistema hace exactamente lo que declara
```

Esa última línea es un `diff` entre `axon seq --events` y `axon trace --seq`.
Corre en CI en cada push.

`axon test` genera el testkit que prueba esos patrones, y corre contra la
implementación real con `node --test`, sin dependencias:

```console
▶ payments · contrato
  ✔ acepta order.placed@v1 tal como lo emite su dueno
  ✔ la segunda entrega de order.placed@v1 no repite el efecto
  ✔ propaga la cadena causal al reaccionar a order.placed@v1
  ✔ nada se publica fuera del outbox
▶ payments · maquina payment
  ✔ cada transicion declarada es legal desde sus estados de origen
  ✔ una transicion no declarada revienta
```

El ejemplo honra los patrones que declara, no los simula: el pago se escribe en
la misma transacción que su evento (outbox real en Postgres, publicado por un
relay), reentregar el mismo envelope no duplica el cobro (inbox real), y la
transición de estado la impone `paymentNext()`, que revienta si no está en el
manifiesto. Los adaptadores de infraestructura son
[150 líneas](examples/services/runtime.ts) — eso es todo lo que axon deja
deliberadamente en manos de quien despliega.

Ninguno de los dos es una fuente de verdad nueva. El **API gateway** sale de los
métodos que cada servicio ya declara con `http`; los **buckets**, de un bloque que
decide una sola cosa importante:

```toml
[methods.placeOrder]
http       = "POST /v1/orders"
auth       = "public"      # obligatorio: el edge falla cerrado
rate_limit = 60            # obligatorio si es pública
timeout_ms = 5000

[infra.buckets.recibos]
retention_days = 2555      # 7 años; sin esto un bucket crece para siempre

[infra.buckets.assets]
public    = true           # público ⇒ CDN, y sin él no lo es
cache_ttl = 86400
```

`auth` **no tiene default**: una ruta expuesta sin decidir quién puede llamarla es un
incidente esperando ocurrir, así que `verify` la bloquea. Y una ruta pública sin
`rate_limit` también — el edge no tiene con qué frenar un abuso.

`public = true` en un bucket es una sola decisión con dos consecuencias que van
siempre juntas: lectura anónima **y** CDN delante. Un bucket privado no lleva CDN, y
uno público no queda sin cache. El nombre del bucket cambia en cada entorno, así que
viaja al contenedor como `BUCKET_<NOMBRE>` — la misma variable en los cuatro targets,
apuntando a MinIO en local.
