<h1 align="center">axon</h1>

<p align="center">
  <em>El manifiesto es la fuente de verdad. El código, la infraestructura y los
  diagramas son proyecciones. <code>axon verify</code> falla cuando dejan de coincidir.</em>
</p>

<p align="center">
  <a href="https://andrew-tellez.github.io/axon/"><strong>Documentación</strong></a>
</p>

<p align="center">
  <a href="https://github.com/Andrew-Tellez/axon/actions/workflows/ci.yml"><img src="https://github.com/Andrew-Tellez/axon/actions/workflows/ci.yml/badge.svg" alt="ci"></a>
  <img src="https://img.shields.io/badge/dependencias%20en%20runtime-0-brightgreen" alt="cero dependencias">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT"></a>
</p>

```sh
curl -fsSL https://raw.githubusercontent.com/Andrew-Tellez/axon/main/install.sh | sh
```

Un binario. Sin runtime, sin Node, sin Python, sin JVM. Corre igual en tu laptop y en un contenedor de CI vacío.

---

## El problema

Preguntá en cualquier equipo con veinte microservicios: *¿quién consume este evento
y qué se rompe si le cambio un campo?* La respuesta honesta es "hay que leer cinco
repos". Los frameworks actuales no ayudan porque viven dentro de un lenguaje
(NestJS, Spring, Micronaut) o son un runtime que hay que desplegar y operar (Dapr).

Ninguno sabe que el `order.placed@v1` que emite un servicio en Go es el mismo que
consume un servicio en Kotlin. Esa relación existe solo en la cabeza del equipo,
hasta que alguien renuncia.

## La idea

Declarás el servicio una vez. Todo lo demás se deriva:

```
  asyncapi.yaml ─────┐                     (axon import)
                     │
                     ├─ axon build      código: contratos + clase base
                     ├─ axon test       pruebas: unitarias, integración, e2e
                     ├─ axon openapi    OpenAPI 3.1 de toda la plataforma
                     ├─ axon infra      IaC: local · gcp · aws · k8s
manifiesto.toml ─────┼─ axon ci         pipeline con los gates que importan
                     ├─ axon graph      topología de eventos
   fuente de verdad  ├─ axon classes    diagrama de clases
                     ├─ axon er         entidad-relación (de las migraciones)
                     ├─ axon seq        flujo causal esperado
                     ├─ axon trace      flujo causal REAL (debug local)
                     ├─ axon discover   registro de servicios y métodos
                     └─ axon verify     drift: falla en CI
```

Nada de eso se edita a mano. Si el diagrama no coincide con el código, no es que el
diagrama esté viejo: es que alguien rompió el manifiesto, y CI lo dice antes del merge.

## El manifiesto

```toml
service = "payments"
version = "1.2.0"
owner   = "equipo-pagos"     # gobernanza: nada sin dueño
tier    = "0"                # criticidad: decide SLO y alertas

[emits."payment.captured@v1"]        # dominio.hecho@versión, siempre
paymentId = "uuid"
amount    = "money"

[consumes."order.placed@v1"]
handler = "onOrderPlaced"

[methods.capturePayment]
http       = "POST /v1/payments"
idempotent = true                    # obligatorio si muta
in  = { orderId = "uuid", amount = "money" }
out = { paymentId = "uuid" }

[[depends]]
service    = "orders"
method     = "getOrder"
timeout_ms = 1000                    # obligatorio: sin presupuesto, no hay llamada
retries    = 3
breaker    = true

[patterns]
outbox = true

[infra]
state      = "postgres"
migrations = "sql/payments/"
secrets    = ["STRIPE_API_KEY"]

[env.prod]                           # los entornos son deltas, no copias
min_instances = 3
```

Tipos: `string int float bool timestamp uuid json money`. `money` es un tipo propio
a propósito — un `float` para dinero es un bug esperando su turno.

## Entrar sin reescribir nada

Si el equipo ya tiene un catálogo de eventos, el manifiesto no se escribe a mano:

```console
$ axon import asyncapi eventos.yaml > manifests/shipping.toml
$ axon verify manifests/
error: shipping-service: sin `owner`; un servicio sin dueno no se despliega
error: shipping-service: sin `tier`; la criticidad decide alertas y SLO
```

Lee AsyncAPI **2.x y 3.x**, en JSON o YAML, y traduce la semántica invertida de 2.x
correctamente (`publish` es lo que la app *recibe*, `subscribe` lo que *emite* — al
revés de lo que sugieren las palabras). Mapea `format: uuid`, `date-time` y detecta
`{amount, currency}` como `money`.

Lo que el documento no dice — dueño, criticidad, timeouts — sale como `TODO`, y
`verify` lo reclama: **un placeholder no es un valor**. El import te deja en un
estado incompleto pero honesto, nunca en uno que finge estar listo.

## Patrones: declarados, no recordados

Un patrón que hay que acordarse de aplicar no es un patrón: es una convención que
alguien va a romper a las 3am. En axon el patrón se declara y el compilador lo emite.
Si no está en el código generado, no está.

| Patrón | Se declara | Qué produce |
| --- | --- | --- |
| **Transactional outbox** | `[patterns] outbox = true` | Los emisores escriben en el outbox; `bus.publish` **desaparece** del código generado. Adiós dual-write. |
| **Consumidor idempotente** | siempre | `dispatch()` deduplica por id de envelope antes de rutear. |
| **Cadena causal** | siempre | `traceparent` + `correlationId` + `causationId` propagados por el emisor. |
| **Dead letter** | siempre | Suscripción con DLQ en los cuatro targets. No hay forma de declarar un consumidor sin ella. |
| **Database per service** | `[infra] state` | Una base por servicio, y `verify` bloquea cualquier FK que cruce el límite. |
| **Circuit breaker / timeouts** | `[[depends]]` | `timeout_ms` obligatorio; reintentar algo no idempotente es un error. |
| **Idempotency-Key** | `idempotent = true` | Header obligatorio en el OpenAPI de todo método mutante. |
| **RFC 7807** | siempre | Un solo formato de error en toda la plataforma, con el `traceId` dentro. |
| **Expand / migrate / contract** | nombre del archivo | Un `DROP` fuera de un `.contract.sql` es un error de `verify`. |

Los patrones GoF viven un nivel abajo, en el código que escribe el equipo — para eso
está [`gof-patterns`](https://github.com/Andrew-Tellez/patterns), en seis lenguajes.
axon se ocupa de los **arquitectónicos**: los que cruzan procesos y que ninguna
librería dentro de un lenguaje puede garantizar sola.

## Qué se declara y qué no

La lógica de negocio la escribe la persona. Siempre. Un manifiesto que intente
expresar *todo* el comportamiento termina siendo un lenguaje de programación nuevo
y peor que los seis a los que compila — es exactamente donde murieron MDA, Rational
Rose y el low-code.

Pero hay una franja que **sí** es idéntica en todos los lenguajes y que hoy vive
dispersa en `if`s: **las reglas de decisión, no la implementación.**

```toml
[machine.payment]
initial = "pending"
final   = ["refunded", "failed"]

[machine.payment.transitions.capture]
from  = ["pending"]
to    = "captured"
on    = "capturePayment"           # el método que la dispara
emits = "payment.captured@v1"      # el evento que sale al completarla

[machine.payment.transitions.refund]
from        = ["captured"]
to          = "refunded"
on          = "refundPayment"
compensates = "capture"            # la inversa, para sagas
```

`axon build` genera la tabla de transiciones exhaustiva y tipada — `PaymentState`,
`paymentCan()`, `paymentNext()` que revienta ante una transición ilegal. `axon states`
la dibuja. Y `axon verify` **prueba propiedades sobre ella** antes del merge:

- un estado inalcanzable desde el inicial
- un estado no final sin salida — un deadlock, encontrado en CI y no en producción
- una transición disparada por un método o evento que no existe
- una transición que emite un evento que el servicio no declara emitir
- una compensación que apunta a un paso inexistente

**El qué es portable; el cómo no.** El cuerpo de `capturePayment` — cobrar en Stripe,
decidir si falla, escribir la fila — es tuyo, en tu lenguaje, en un `extends` de la
clase generada. axon se queda con la parte que se puede verificar.

## Resiliencia y CAP: del diseño alto al bajo

El manifiesto es diseño de alto nivel — límites de servicio, topología, qué garantiza
cada uno. El compilador lo baja a diseño de bajo nivel: nivel de aislamiento, política
de reintentos, firmas de método. **Eso es todo el proyecto en una frase.**

```toml
[[depends]]
service    = "orders"
method     = "getOrder"
timeout_ms = 1000        # obligatorio
retries    = 3           # solo si el otro método es idempotente
breaker    = true

[cap]
consistency  = "strong"    # el dinero no admite un saldo viejo
on_partition = "reject"    # antes de servir algo viejo, no sirve nada
```

`axon build` emite el cliente con esa política ejecutándose: timeout, backoff
exponencial **con jitter completo** (sin jitter todos los clientes reintentan a la vez
y el otro lado nunca se levanta), y un circuito por destino que pasa a medio abierto
tras el enfriamiento. Los reintentos solo se emiten para métodos idempotentes — `verify`
bloquea el resto — y la llamada lleva `traceparent`, `x-correlation-id`,
`x-causation-id` e `idempotency-key`.

### El lado del teorema que sí se elige

La tolerancia a particiones no es una opción: la red se parte. Lo que se elige es qué
hacer mientras está partida, y esa decisión **cambia el código**:

| | `strong` / `reject` (CP) | `eventual` / `degrade` (AP) |
| --- | --- | --- |
| `nivelAislamiento` | `SERIALIZABLE` | `READ COMMITTED` |
| Obsolescencia | — | `obsolescenciaMaximaMs`, obligatoria |
| Firma del cliente | `(input, e)` | `(input, e, respaldo)` |

La última fila es la que importa: si declarás `degrade`, el cliente generado **exige**
un parámetro `respaldo`. No podés decir "elijo disponibilidad" y después no escribir
qué se sirve cuando el otro lado no está. No es una convención — no compila.

`verify` bloquea `strong` + `degrade` (es la contradicción del teorema), exige
`max_staleness_ms` en todo `eventual` (sin un número, "eventual" es una palabra), y
avisa cuando un servicio `strong` llama sincrónicamente a uno `eventual`: **la garantía
de la ruta es la del eslabón más débil, no la tuya.**

## Trazabilidad desde el día uno

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

## OpenTelemetry sin pegamento

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

## Agnóstico del cloud

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

### La demo, en dos comandos

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

## El edge y el almacenamiento

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

## Seguridad

Cada regla cita su categoría del [OWASP Top 10 (2021)](https://owasp.org/Top10/),
porque un error que no dice *por qué* importa se silencia con un allow:

| | Regla | |
| --- | --- | --- |
| **A01** | Ruta pública que muta en un servicio `tier = "0"` | error |
| **A01** | Tabla sin la columna del inquilino cuando hay `tenant_column` | error |
| **A02** | Un secreto literal donde va su nombre | error |
| **A04** | Ruta pública sin `timeout_ms` | error |
| **A05** | Bucket público sin `retention_days` | aviso |
| **A08** | `[ci].image` sin digest — una etiqueta es mutable | aviso |
| **A09** | Un campo declarado `pii` devuelto por una ruta pública | error |

Y lo que no se avisa, se **genera endurecido**:

- **A05** — el `Deployment` de k8s sale con `runAsNonRoot`, `readOnlyRootFilesystem`,
  `capabilities: drop ALL`, `seccompProfile: RuntimeDefault` y sin token de service
  account montado. Más una `NetworkPolicy` de denegación por defecto: al pod solo entra
  el edge.
- **A01** — un servicio sin ninguna ruta pública se despliega con
  `ingress = INTERNAL_LOAD_BALANCER`: no tiene puerta a internet aunque alguien se
  equivoque en el gateway.
- **A08** — el pipeline construye la imagen, toma su digest y **despliega por digest**,
  con `provenance: true`.
- **A09** — `axon build` emite `camposPII` y un `redactar()` recursivo. Un dato personal
  se filtra por un log, no por un exploit.

### RLS y enmascarado

```toml
pii = ["customer_email"]

[infra]
tenant_column = "tenant_id"
tenant_exempt = ["auditoria"]   # lo que no es de negocio
```

`axon rls` cruza dos cosas que axon ya sabe — el esquema real (leído de las migraciones
con un parser SQL) y los campos `pii` — y emite una migración más:

```sql
ALTER TABLE "order" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "order" FORCE ROW LEVEL SECURITY;   -- también al dueño de la tabla
CREATE POLICY "order_inquilino" ON "order"
  USING ("tenant_id" = current_setting('axon.tenant', true)::uuid)
  WITH CHECK ("tenant_id" = current_setting('axon.tenant', true)::uuid);

CREATE OR REPLACE VIEW "order_enmascarada" AS SELECT
  id, customer_id, total_cents, status, tenant_id,
  '[redactado]'::text AS "customer_email"
FROM "order";
REVOKE ALL ON "order" FROM axon_lectura;
GRANT SELECT ON "order_enmascarada" TO axon_lectura;
```

La regla de `verify` es la que importa: **una tabla que se olvida de la columna del
inquilino no recibe política, y una tabla sin política no falla — devuelve las filas de
todos.** Ese es el modo de fallo silencioso que la declaración elimina.

El suite no lee ese SQL: lo aplica a un Postgres real y comprueba que sin inquilino se
ven 0 filas, que cada inquilino ve solo la suya, que escribir para otro se rechaza, y
que la vista devuelve `[redactado]`.

### Una copia enmascarada, con `pg_anon`

Las vistas protegen la **consulta viva**: un rol de analítica nunca ve el dato crudo.
Para el otro problema —darle datos realistas a staging, a soporte o a un tercero que no
debe ver los de verdad— hace falta una **copia** enmascarada, y eso lo hace
[`pg_anon`](https://github.com/TantorLabs/pg_anon) (que no es una extensión de Postgres,
sino un CLI que clona la base reemplazando los campos en el camino).

Su mayor friccón es mantener el diccionario de campos sensibles, y su `create-dict` lo
detecta **por heurística**. axon lo emite declarado:

```sh
axon rls manifests/ --target pg_anon > sens_dict.py
pg_anon --mode=dump --prepared-sens-dict-file=sens_dict.py ...
```

La regla sale del **tipo** de cada columna, nunca de su nombre: adivinar porque una
columna se llama `email` falla con `correo` y se equivoca con `email_template`. Lo que
axon garantiza es la **cobertura** —ningún campo declarado `pii` se queda sin regla—; la
regla en sí es tuya y se edita, por ejemplo para preservar la forma de un correo.

Las tablas del propio framework se excluyen del dump: el `outbox` lleva el payload de
cada evento en un `jsonb`, que es el último lugar donde uno buscaría una fuga.

El suite aplica cada regla generada a una columna de su tipo en un Postgres real —un
cast que falta hace fallar el dump a mitad de camino— y comprueba que el dato original
no sobreviva a ninguna.

## Gobernanza

`axon.policy.toml`, versionado junto al código:

```toml
require_owner          = true
require_tier           = true
allowed_event_prefixes = ["order", "payment", "billing"]
max_deps_per_service   = 7    # si lo pasás, es un monolito distribuido

[ci]                          # el layout del repo es del equipo, no de axon
service_dir = "services/{service}"
test_cmd    = "make -C services/{service} test"
```

Lo que `axon verify` bloquea hoy:

| Chequeo | |
| --- | --- |
| Se consume un evento que nadie emite | error |
| Dos emisores del mismo evento con esquemas distintos | error |
| Se llama un método que el otro servicio no expone | error |
| Dependencia sin `timeout_ms` | error |
| Reintentos sobre un método no idempotente | error |
| Ruta HTTP sin versión, o duplicada entre servicios | error |
| Método mutante sin `idempotent` | error |
| FK que cruza el límite de un servicio | error |
| Migración destructiva sin `.contract.sql` | error |
| Servicio sin `owner` o sin `tier` | error |
| Evento sin consumidores · reintentos sin breaker · demasiadas deps síncronas | aviso |

## Plugins

Un plugin es **cualquier ejecutable en el `PATH` llamado `axon-*`**. Recibe JSON por
stdin, escribe por stdout. Sin ABI, sin cargar librerías, sin versiones que casen:
el modelo de `git` y de `protoc`. Puede ser un binario de Go o tres líneas de shell.

| Clase | Se invoca con | Recibe | Devuelve |
| --- | --- | --- | --- |
| `axon-gen-<lang>` | `axon build --lang go` | `{manifest, peers}` | código fuente |
| `axon-infra-<target>` | `axon infra --target pulumi` | el plan neutral | IaC |
| `axon-check-<regla>` | `axon verify` (todos, siempre) | todos los manifiestos | `[{level, message}]` |

Una regla de gobernanza propia, completa:

```sh
#!/bin/sh
# axon-check-nombres — ningún servicio se llama "service" o "api"
jq -c '[.[] | select(.service|test("^(service|api)$"))
        | {level:"error", message:("\(.service): nombre genérico prohibido")}]'
```

```console
$ chmod +x axon-check-nombres && mv axon-check-nombres ~/.local/bin/
$ axon verify manifests/
error: [axon-check-nombres] api: nombre generico prohibido
```

Bloquea el pipeline exactamente igual que una regla nativa.

### `axon-gen-go`, el generador de referencia

[`plugins/axon-gen-go`](plugins/axon-gen-go) es un generador completo escrito **en Go**
— no importa nada de axon, su único contrato es el JSON de stdin. Sirve de plantilla
para cualquier otro lenguaje:

```sh
go build -o ~/.local/bin/axon-gen-go ./plugins/axon-gen-go
axon build manifests/payments.toml manifests/ --lang go > payments/axon.go
```

Produce Go idiomático, no TypeScript traducido: interfaz de handlers en vez de herencia,
`ctx` primero y `error` al final, `OrderID` y no `OrderId`, y la salida pasa por
`go/format` antes de salir — un generador no debería dejar código que alguien tenga que
formatear después. El suite comprueba que lo generado pase `go vet`.

Recibe `{manifest, peers}` porque el esquema de un evento consumido lo declara su
emisor: sin los demás manifiestos, ningún generador puede tipar lo que su servicio
recibe.

## Qué comprueba el suite

Los generadores se validan con la herramienta real del ecosistema, no con asserts
propios — un compilador que solo se verifica a sí mismo produce salida inválida:

| | |
| --- | --- |
| El TypeScript generado | `tsc --strict --noEmit` |
| El Terraform generado | `terraform validate` con los providers reales (gcp y aws), sin advertencias |
| El workflow generado | parseo YAML, bloques escalares, y que ningún target filtre otro cloud |
| El testkit generado | `node --test` contra el servicio de ejemplo real |
| El Go generado | `go vet` |
| El DDL | `PARTITION BY`, constraints de tabla, y fallo ruidoso ante SQL inválido |
| La RLS generada | se aplica a un Postgres real y se comprueba que aísla |
| Los cuatro targets | despliegan el workload y entregan a alguien |

```sh
cargo test --release      # 15 checks de conformidad
```

## Estado

Preview. La superficie de comandos es estable; el formato del manifiesto todavía puede
cambiar antes de `v1`.

Saltado a propósito, y cuándo agregarlo:

- **Un generador nativo (TS)** — los demás por plugin, hasta que haya un segundo
  servicio real en otro lenguaje que justifique traerlo al core.
- **Un solo dialecto SQL (PostgreSQL)** — el esquema se lee con `sqlparser`, no con
  una regex, así que aguanta `PARTITION BY`, restricciones a nivel de tabla y lo que
  genere cualquier ORM. Un archivo que no parsea es un error, nunca un silencio.
- **Un solo modelo de ejecución (`container`)** — cualquier otro valor de `runtime` es
  un error de `verify`, no un campo ignorado en silencio. Otro modelo entra por un
  `axon-infra-*`.
- **`verify` compara declaraciones, no el cloud desplegado** — el drift contra el
  state de Terraform llega cuando haya algo desplegado que verificar.
- **Sin runtime propio** — `Bus`, `Inbox` y `Outbox` son interfaces de una línea; el
  adaptador lo pone quien despliega. Un paquete runtime cuando el mismo adaptador se
  repita en tres servicios.

## Desarrollo

```sh
cargo test --release      # 15 checks (los de tsc y terraform se saltan si no están)
cargo run -- verify examples
```

[Documentación](https://andrew-tellez.github.io/axon/) · [Diseño y decisiones](DESIGN.md) · [Referencia de comandos](docs/cli.md) · MIT
