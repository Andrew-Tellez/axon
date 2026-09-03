# axon

Framework de backend para microservicios y arquitecturas event-driven,
**independiente del lenguaje**. Un manifiesto por servicio es la fuente de
verdad; el código, la infraestructura, los contratos y la topología son
proyecciones de él.

```
manifiesto.toml ─┬─ axon build --lang ts   → contratos + clase base (envelope, handlers, emisores)
                 ├─ axon infra             → terraform (topics, subs, DLQ, DB, secretos)
                 ├─ axon graph             → mermaid: topología de eventos
                 ├─ axon classes           → mermaid: diagrama de clases
                 ├─ axon er                → mermaid: entidad-relación (desde las migraciones)
                 ├─ axon seq <evento>      → mermaid: flujo causal esperado
                 ├─ axon discover          → registro de servicios y sus métodos (local o en vivo)
                 └─ axon verify            → drift: falla en CI cuando dejan de coincidir
```

## Diagramas

Ninguno se dibuja a mano y ninguno introduce una fuente de verdad nueva:

- **Clases** (`axon classes`) — proyección directa del manifiesto: servicios, eventos como
  clases, handlers, emisores, dependencias síncronas, y qué patrones implementa cada uno.
- **ER** (`axon er`) — se **introspecta** de las migraciones, no se declara. Meter columnas
  en el manifiesto sería el problema del dual-write disfrazado de documentación. El
  manifiesto solo aporta lo que las migraciones no saben: de qué servicio es cada tabla.
- **Secuencia** (`axon seq order.placed@v1`) — recorre la cadena causal declarada: quién
  consume, a quién llama, qué emite después. Es el flujo *esperado*; el `causationId` de
  los envelopes reales dice el que ocurrió. Diferenciarlos es el siguiente chequeo de drift.

## Por qué existe

Los frameworks existentes son librerías dentro de un lenguaje (NestJS, Spring,
Micronaut) o un runtime que hay que desplegar (Dapr). Ninguno responde
"¿quién consume este evento y qué pasa si le cambio un campo?" sin leer código
de cinco repos. axon lo responde porque esa relación está declarada, no inferida.

Tres cosas no son opcionales, y por eso las genera el compilador en vez de
dejarlas a la disciplina del equipo:

1. **Trazabilidad desde el día uno.** Todo mensaje viaja en un envelope
   CloudEvents extendido con `traceparent` (W3C), `correlationId` (estable en
   todo el flujo de negocio) y `causationId` (el mensaje que lo provocó). El
   emisor generado recibe el mensaje causante y propaga la cadena: no hay forma
   de publicar un evento huérfano sin salirse del framework.
2. **Descubrimiento.** Cada servicio sirve su propio manifiesto en
   `/.well-known/axon.json`. `axon discover <dir|url>` fusiona manifiestos de
   disco y de servicios vivos en un registro con métodos, entradas, salidas y
   eventos. Los servicios externos (Stripe, un ERP) se congelan en un
   `*.external.toml` — se descubren una vez, quedan versionados como contrato.
3. **Infraestructura como código.** El bloque `[infra]` y los eventos declarados
   producen el terraform: un topic por evento, una suscripción por consumidor,
   DLQ siempre, la base de datos del servicio y sus contenedores de secretos.
   No hay topic sin dueño ni consumidor sin DLQ porque no hay forma de escribirlo.

## Patrones: declarados, no recordados

Un patrón que hay que acordarse de aplicar no es un patrón, es una convención que
alguien va a romper a las 3am. En axon el patrón se declara y el compilador lo emite;
si no está en el código generado, no está.

| Patrón | Se declara | Qué genera |
| --- | --- | --- |
| **Transactional outbox** | `[patterns] outbox = true` | Los emisores escriben en el outbox, no en el bus — el `publish` directo deja de existir. Terraform crea la tabla y el usuario del relay. Adiós dual-write. |
| **Consumidor idempotente (inbox)** | siempre | `dispatch()` deduplica por `id` de envelope antes de rutear. El broker entrega al menos una vez; el efecto ocurre una sola. |
| **Envelope / cadena causal** | siempre | `traceparent`, `correlationId`, `causationId` propagados por el emisor generado. |
| **Dead letter** | siempre | Suscripción con `dead_letter_policy` y su topic. No hay forma de declarar un consumidor sin DLQ. |
| **Database per service** | `[infra] state` | Una base por servicio, nunca compartida. |
| **Contratos versionados** | `evento@vN` | `verify` bloquea el cambio de esquema de una versión publicada. |

Los patrones GoF viven un nivel abajo, en el código que escribe el equipo — para eso
está [`gof-patterns`](https://github.com/Andrew-Tellez/patterns), en los seis lenguajes.
axon no los reimplementa: se ocupa de los patrones *arquitectónicos*, los que cruzan
procesos y que ninguna librería dentro de un lenguaje puede garantizar sola.

## Migraciones

axon **no** es una herramienta de migraciones — Flyway, Alembic y golang-migrate ya
existen y son mejores en eso. Lo que axon hace es tratarlas como la fuente de verdad
del esquema y verificar lo que ellas no pueden ver:

```toml
[infra]
migrations = "sql/payments/"
```

```
sql/payments/
  001_payment.expand.sql       expand:   aditivo, compatible hacia atrás
  002_provider_ref.expand.sql  expand:   columna nueva nullable
  003_drop_legacy.contract.sql contract: destructivo, y lo dice en el nombre
```

- El esquema es la suma de las migraciones plegadas en orden. No hay un `schema.sql`
  duplicado que se desincronice; el ER sale de aquí.
- **Expand → migrate → contract** es obligatorio, no una recomendación: una migración
  con `DROP` que no se llame `.contract.sql` es un error de `verify`. Desplegar un
  destructivo junto al código que deja de usar la columna rompe el rollback.
- **Ninguna FK cruza el límite de un servicio.** `verify` lo bloquea: se guarda el id,
  y la consistencia entre servicios se resuelve con eventos, no con el motor de la base.
- Prefijo numérico obligatorio, o el orden no es determinista.

## Verificación de drift

`axon verify` es lo que convierte el manifiesto en algo más que documentación:

| Chequeo | Resultado |
| --- | --- |
| Se consume un evento que nadie emite | error |
| Dos servicios emiten el mismo evento con esquemas distintos | error |
| Se depende de un método que el otro servicio no expone | error |
| Se emite un evento sin consumidores | aviso |
| FK que cruza el límite de un servicio | error |
| Migración destructiva sin marcar `.contract.sql` | error |
| Migración sin prefijo numérico | aviso |

En CI, contra los manifiestos vivos (`axon verify https://orders/... https://payments/...`),
compara lo declarado con lo desplegado.

## Manifiesto

```toml
service = "payments"
version = "1.2.0"

[emits."payment.captured@v1"]      # nombre@versión, siempre
paymentId = "uuid"
amount    = "money"

[consumes."order.placed@v1"]
handler = "onOrderPlaced"

[methods.capturePayment]
in  = { orderId = "uuid", amount = "money" }
out = { paymentId = "uuid" }

[[depends]]
service = "orders"
method  = "getOrder"

[infra]
state   = "postgres"
runtime = "cloudrun"
secrets = ["STRIPE_API_KEY"]
```

Tipos: `string int float bool timestamp uuid json money`. `money` es un tipo
propio a propósito: un float para dinero es un bug esperando su turno.

## Convenciones

- Eventos en pasado y versionados: `dominio.hecho@vN`. Cambio incompatible = `@vN+1`,
  nunca editar el esquema de una versión publicada.
- Un servicio es dueño de los eventos que emite. Nadie más los emite.
- La comunicación síncrona (`methods`) se declara igual que la asíncrona; si no
  está en `depends`, la llamada no debería existir.
- El código generado no se edita. Se hereda de la clase base y se implementan
  los abstractos.

## Estado

Slice ejecutable, sin dependencias (`tomllib`, py3.11+). `python3 test_axon.py`.

Saltado a propósito, y cuándo agregarlo:
- **Un solo target de código (TS)** — otro lenguaje es otra función `build_X`, no un
  motor de plantillas. Agregar cuando exista el segundo servicio real en otro lenguaje.
- **Un solo target de IaC (Pub/Sub + Cloud SQL)** — Kafka/SNS/Rabbit cuando haya un
  despliegue que lo pida.
- **Sin runtime propio** — el `Bus` es una interfaz de tres líneas; el adaptador lo
  pone quien despliega. Agregar un paquete runtime cuando se repita el mismo adaptador
  en tres servicios.
- **`verify` compara manifiestos entre sí, no contra la nube** — el drift contra
  terraform state o contra los topics reales llega cuando haya algo desplegado.
