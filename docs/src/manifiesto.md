# El manifiesto

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
