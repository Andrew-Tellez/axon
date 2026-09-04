<h1 align="center">axon</h1>

<p align="center">
  <em>El manifiesto es la fuente de verdad. El código, la infraestructura y los
  diagramas son proyecciones. <code>axon verify</code> falla cuando dejan de coincidir.</em>
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

## Agnóstico del cloud

El manifiesto no menciona ningún proveedor. `axon infra` produce primero un **plan
neutral** y después lo renderiza:

| Target | Mensajería | Cómputo | Estado y secretos |
| --- | --- | --- | --- |
| `local` | NATS JetStream | tus servicios con `build:` | Postgres por servicio + migraciones aplicadas |
| `gcp` | Pub/Sub con push y DLQ | Cloud Run + service account | Cloud SQL + Secret Manager |
| `aws` | SNS → SQS con redrive | ECS Fargate + autoscaling | RDS + Secrets Manager |
| `k8s` | Knative Broker/Trigger | Deployment + Service + HPA | External Secrets |
| `plan` | — | — | El plan neutral en JSON, para renderizarlo vos mismo |

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

El ejemplo honra los patrones que declara, no los simula: el pago se escribe en
la misma transacción que su evento (outbox real en Postgres, publicado por un
relay), reentregar el mismo envelope no duplica el cobro (inbox real), y la
transición de estado la impone `paymentNext()`, que revienta si no está en el
manifiesto. Los adaptadores de infraestructura son
[150 líneas](examples/services/runtime.ts) — eso es todo lo que axon deja
deliberadamente en manos de quien despliega.

## Gobernanza

`axon.policy.toml`, versionado junto al código:

```toml
require_owner          = true
require_tier           = true
allowed_event_prefixes = ["order", "payment", "billing"]
max_deps_per_service   = 7    # si lo pasás, es un monolito distribuido
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
| El Terraform generado | `terraform fmt -check` (gcp y aws) |
| El workflow generado | parseo YAML y bloques escalares |
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

[Diseño y decisiones](DESIGN.md) · [Referencia de comandos](docs/cli.md) · MIT
