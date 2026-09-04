# axon

> El manifiesto es la fuente de verdad. El código, la infraestructura, las pruebas y los
> diagramas son **proyecciones**. `axon verify` falla en CI cuando dejan de coincidir.

Un binario en Rust, sin runtime. Cuatro targets de infraestructura desde un plan
neutral. Trazabilidad obligatoria por construcción. Patrones impuestos por generación,
no por disciplina.

```sh
curl -fsSL https://raw.githubusercontent.com/Andrew-Tellez/axon/main/install.sh | sh
```

Preguntá en cualquier equipo con veinte microservicios: *¿quién consume este evento
y qué se rompe si le cambio un campo?* La respuesta honesta es "hay que leer cinco
repos". Los frameworks actuales no ayudan porque viven dentro de un lenguaje
(NestJS, Spring, Micronaut) o son un runtime que hay que desplegar y operar (Dapr).

Ninguno sabe que el `order.placed@v1` que emite un servicio en Go es el mismo que
consume un servicio en Kotlin. Esa relación existe solo en la cabeza del equipo,
hasta que alguien renuncia.

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
