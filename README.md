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

Preguntá en cualquier equipo con veinte microservicios: *¿quién consume este evento y
qué se rompe si le cambio un campo?* La respuesta honesta es «hay que leer cinco repos».

Los frameworks actuales no ayudan porque viven dentro de un lenguaje (NestJS, Spring,
Micronaut) o son un runtime que hay que desplegar y operar (Dapr). Ninguno sabe que el
`order.placed@v1` que emite un servicio en Go es el mismo que consume un servicio en
Kotlin. Esa relación existe solo en la cabeza del equipo, hasta que alguien renuncia.

## La idea

Declarás el servicio una vez, y todo lo demás se deriva:

```
                     ┌─ axon build      contratos, clase base, clientes resilientes
  asyncapi.yaml ─────┤                     (axon import)
                     ├─ axon test       testkit: contrato, idempotencia, máquinas
                     ├─ axon openapi    OpenAPI 3.1 de toda la plataforma
                     ├─ axon infra      IaC: local · gcp · aws · k8s
manifiesto.toml ─────┼─ axon rls        RLS por fila y vistas enmascaradas
                     ├─ axon flags      configuración de flagd (OpenFeature)
   fuente de verdad  ├─ axon ci         pipeline: gates de axon, deploy del target
                     ├─ axon load       carga con umbrales del manifiesto
                     ├─ axon graph · classes · er · states · seq   diagramas
                     ├─ axon trace      la cadena causal REAL, para debug local
                     ├─ axon cap        qué implica el lado CAP que elegiste
                     └─ axon verify     drift: falla en CI
```

Nada de eso se edita a mano. Si el diagrama no coincide con el código, no es que el
diagrama esté viejo: es que alguien rompió el manifiesto, y CI lo dice antes del merge.

> **El manifiesto es diseño de alto nivel** —límites de servicio, topología, qué
> garantiza cada uno— **y el compilador lo baja a diseño de bajo nivel**: nivel de
> aislamiento, política de reintentos, firmas de método, recursos de infraestructura. Y
> verifica que sigan de acuerdo.

## La demo, en dos comandos

`examples/` trae tres servicios que corren de verdad —uno sobre cuatro nodos de Postgres
con [pgdog](https://pgdog.dev) delante, y otro coordinando una saga. `./demo.sh` levanta
el sistema completo y comprueba **nueve** cosas contra la realidad:

```console
$ cd examples && ./demo.sh
==> cadena causal real
└─ POST /v1/orders <- http
   └─ order.placed@v1 <- orders
      └─ payment.captured@v1 <- payments

==> la traza en OpenTelemetry
  OK: 5 spans, un raiz, sin huerfanos, cruzando ['orders', 'payments']

==> esperado (manifiesto) vs real (log de envelopes)
  OK: el sistema hace exactamente lo que declara

==> aislamiento por inquilino a traves del pooler
  OK: pgdog rechaza en el router la consulta sin inquilino
  OK: 20 de 20 conexiones vieron 1 fila propia y 0 del inquilino ajeno

==> la saga: compensacion y retome, medidos
  OK: el cobro se deshizo y al comercio no se le pago
  OK: retomada desde el diario, compensada, y el reembolso alcanzo al cobro

==> event sourcing y CQRS, medidos
  OK: dos escrituras a la misma version: una entro, el UNIQUE rechazo la otra
  OK: 0ms de atraso de la vista, dentro del presupuesto declarado
  OK: el relay volvio y publico lo pendiente; nadie lo reintento a mano
  OK: la foto dice lo mismo que la proyeccion, que se construyo sin usarla

==> reintentos declarados vs ocurridos
  OK: 3 llamadas = 1 + 2 reintentos, exactamente lo declarado
  OK: 14000ms dentro del presupuesto de 60000ms

==> rollout declarado vs aplicado
  declarado 10%  medido 10.7%  (32 de 300)
  OK: estable por inquilino, y el porcentaje aplica

==> la bodega: esquema, embudo y PII
  OK: 12 flujos, 12 llegaron al cobro (conversion 100%)
  OK: 12 hasheados, 0 correos en claro

==> capacidad declarada vs medida
  axon: 0 umbrales incumplidos
```

Corre en CI en cada push.

## Con qué está hecho, y con qué se verifica

**axon no necesita nada para correr**: un binario en Rust, sin runtime. Las herramientas
de abajo son las que usa lo que *genera*, y cada una solo para su parte — si falta, se
salta esa parte y no el resto.

La lista importa por una razón: **un generador no se valida con asserts propios, se
valida con la herramienta real de su ecosistema.** Los tres primeros generadores de este
proyecto producían salida inválida y el suite no lo veía, porque axon se verificaba
únicamente contra sí mismo.

| Herramienta | Para qué | Cómo se verifica lo generado |
| --- | --- | --- |
| **Docker** | `--target local`: broker, Postgres por servicio, MinIO, Jaeger, flagd, el edge y tus servicios | `./demo.sh` levanta el sistema y comprueba cuatro cosas contra la realidad |
| **Terraform** | `--target gcp` y `--target aws` | `terraform validate` con los **providers reales**, y sin advertencias |
| **`tsc`** | el TypeScript de `axon build` y `axon test` | `tsc --strict --noEmit`, más el typecheck del servicio de ejemplo |
| **Node 24+** | corre el testkit sin paso de build, con type-stripping | `node --test` contra el servicio de ejemplo real |
| **Go** | `axon-gen-go`, el generador de referencia de plugins | `go vet` sobre lo emitido, y `go/format` antes de emitirlo |
| **Postgres** | migraciones, RLS, vistas enmascaradas | la RLS se **aplica a un Postgres real** y se comprueba que aísla |
| **`kubectl`** | `--target k8s` | parseo de los 16 objetos que emite |
| **k6** | `axon load`: carga con umbrales del manifiesto | corre en el demo y `--check` diffea lo medido contra lo declarado |
| **OpenTelemetry** | trazas; el envelope ya propaga `traceparent` | el demo verifica el árbol de spans: un raíz, cero huérfanos, dos servicios |
| **OpenFeature / flagd** | `axon flags`: evaluación por OFREP | el demo mide el rollout declarado contra el aplicado |
| **Flyway** | aplica las migraciones; axon las lee, no las ejecuta | `validateMigrationNaming` obligatorio: ignoraba archivos en silencio |
| **BigQuery / Snowflake / ClickHouse** | `axon analytics`, y la ingesta que la lleva | el DDL se parsea con **el dialecto de cada uno**, y el demo carga eventos reales en ClickHouse y comprueba el embudo |
| **pgdog** | `axon pooler`: pooler y sharder, levantado por el target local | el `pgdog.toml` se valida contra su **JSON Schema oficial**, y el demo mide el aislamiento por inquilino a través del pooler |
| **cocogitto** | Conventional Commits y el changelog | el hook rechaza el mensaje antes de crear el commit |
| **mdBook** | esta documentación | cada bloque `toml` de las páginas pasa por `axon verify` |

### Dentro del binario

Seis dependencias, ninguna accidental:

| | |
| --- | --- |
| `clap` | la CLI |
| `serde` + `toml` + `serde_json` + `serde_yaml_ng` | el manifiesto, y AsyncAPI en JSON o YAML |
| `indexmap` | orden de inserción: sin él la salida generada cambia entre corridas y `git diff --exit-code` deja de significar algo |
| `sqlparser` | el esquema sale de las migraciones con un parser SQL de verdad. Una regex se rompe con `PARTITION BY` — y lo peor es que se rompe **en silencio** |
| `ureq` | `axon discover` contra servicios vivos |

`regex` estuvo y se fue: quedaba para comprobar tres dígitos y un guion bajo en el
nombre de un archivo.

## Documentación

**[andrew-tellez.github.io/axon](https://andrew-tellez.github.io/axon/)** — construida
con [mdBook](https://rust-lang.github.io/mdBook/), con búsqueda y una versión archivada
por cada release.

| | |
| --- | --- |
| [Tu primer manifiesto](https://andrew-tellez.github.io/axon/primeros-pasos.html) | Diez minutos, de cero a verificado |
| [Referencia del manifiesto](https://andrew-tellez.github.io/axon/manifiesto.html) | Cada campo y por qué existe |
| [Patrones](https://andrew-tellez.github.io/axon/patrones.html) | Declarados, no recordados: outbox, inbox idempotente, **saga**, **event sourcing**, **CQRS** |
| [CAP y resiliencia](https://andrew-tellez.github.io/axon/cap.html) | El lado que sí se elige |
| [Reglas y drift](https://andrew-tellez.github.io/axon/verificacion.html) | Todo lo que `verify` bloquea |
| [Seguridad](https://andrew-tellez.github.io/axon/seguridad.html) | OWASP, RLS, enmascarado |
| [Plugins](https://andrew-tellez.github.io/axon/plugins.html) | Cualquier ejecutable `axon-*` |

Los ejemplos de manifiesto de esa documentación **no son texto**: el suite extrae cada
bloque y le corre `axon verify`, así que no pueden quedar viejos en silencio.

## Estado

Preview. La superficie de comandos es estable; el formato del manifiesto todavía puede
cambiar antes de `v1`. Ver el
[changelog](CHANGELOG.md) y [qué se comprueba](https://andrew-tellez.github.io/axon/garantias.html).

## Desarrollo

```sh
cargo test --release            # el suite completo
cargo run --release -- verify examples
cd examples && ./demo.sh        # necesita Docker
mdbook serve docs --open        # la documentación
```

[Contribuir](CONTRIBUTING.md) · [Diseño y decisiones](DESIGN.md) · [gof-patterns](https://github.com/Andrew-Tellez/patterns) · MIT
