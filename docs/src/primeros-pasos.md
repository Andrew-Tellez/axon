# Tu primer manifiesto

Diez minutos, sin escribir código de negocio todavía. Al final vas a tener un servicio
verificado, su infraestructura y sus diagramas — todo derivado de un archivo.

## 1. Declarar el servicio

```toml
# manifests/orders.toml
service = "orders"
version = "0.1.0"
owner   = "tu-equipo"      # sin dueño no se despliega
tier    = "2"              # criticidad: decide SLO, alertas y muestreo

[emits."order.placed@v1"]  # dominio.hecho@versión, siempre
orderId    = "uuid"
customerId = "uuid"
total      = "money"       # un float para dinero es un bug esperando su turno

[methods.placeOrder]
http       = "POST /v1/orders"
auth       = "public"      # obligatorio: el edge falla cerrado
rate_limit = 60            # obligatorio si es pública
timeout_ms = 5000
idempotent = true          # obligatorio si muta
in  = { customerId = "uuid", total = "money" }
out = { orderId = "uuid" }
```

## 2. Verificar antes de escribir una línea

```sh
axon verify manifests/
```

Esto ya te va a decir algo. Si omitiste `auth`, si la ruta no lleva versión, si el
método muta sin ser idempotente. **El objetivo es que falle acá y no en producción.**

## 3. Generar el contrato

```sh
axon build manifests/orders.toml manifests/ --lang ts > src/contracts.ts
```

Salen los tipos, el envelope con la cadena causal, la clase base abstracta y —si hay
`[[depends]]`— los clientes con su política de reintentos. Lo que **no** sale es tu
lógica: heredás de la clase y escribís los métodos abstractos.

## 4. Levantar el sistema en tu máquina

```sh
axon infra manifests/ --target local > axon.local.yml
docker compose -f axon.local.yml up -d --wait
```

Eso levanta el broker, una base por servicio, las migraciones aplicadas, el edge, el
almacenamiento de objetos, el backend de trazas y flagd si declaraste flags.

**Local no es un subsistema aparte: es otro render del mismo plan neutral.** Por eso
local y producción no pueden divergir.

## 5. Mirar lo que declaraste

```sh
axon graph   manifests/    # topología de eventos
axon classes manifests/    # diagrama de clases
axon seq     order.placed@v1 manifests/   # el flujo causal esperado
axon cap     manifests/    # qué implica el lado CAP que elegiste
```

Todos emiten Mermaid, que GitHub renderiza en un bloque ` ```mermaid `.

## 6. Registrar los contratos

Cuando publiques:

```sh
axon baseline manifests/ > manifests/axon.baseline.json
```

A partir de ahí, `verify` bloquea cualquier cambio incompatible a una versión ya
publicada. [Una versión publicada es inmutable](./verificacion.md).

## Y ahora sí, el código

Lo único que escribís a mano:

```ts
import { OrdersService, type PlaceOrderIn, type PlaceOrderOut } from "./contracts.ts";

export class Orders extends OrdersService {
  async placeOrder(input: PlaceOrderIn, e: Envelope<unknown>): Promise<PlaceOrderOut> {
    const orderId = crypto.randomUUID();
    await this.db.insertar(orderId, input);
    // `e` es la causa: el emisor generado propaga traceparent y correlationId
    await this.emitOrderPlacedV1({ orderId, ...input }, e);
    return { orderId };
  }
}
```

El ejemplo completo y ejecutable —dos servicios, Postgres, NATS, outbox real,
OpenTelemetry— está en
[`examples/`](https://github.com/Andrew-Tellez/axon/tree/main/examples), y
`./demo.sh` lo levanta y comprueba que la traza real coincida con el manifiesto.
