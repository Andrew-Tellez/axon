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

`examples/` trae dos servicios que corren de verdad. `./demo.sh` levanta el sistema
completo y comprueba **cuatro** cosas contra la realidad:

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

==> rollout declarado vs aplicado
  declarado 10%  medido 10.7%  (32 de 300)
  OK: estable por inquilino, y el porcentaje aplica

==> capacidad declarada vs medida
  axon: 0 umbrales incumplidos
```

Corre en CI en cada push.

## Documentación

**[andrew-tellez.github.io/axon](https://andrew-tellez.github.io/axon/)** — construida
con [mdBook](https://rust-lang.github.io/mdBook/), con búsqueda y una versión archivada
por cada release.

| | |
| --- | --- |
| [Tu primer manifiesto](https://andrew-tellez.github.io/axon/primeros-pasos.html) | Diez minutos, de cero a verificado |
| [Referencia del manifiesto](https://andrew-tellez.github.io/axon/manifiesto.html) | Cada campo y por qué existe |
| [Patrones](https://andrew-tellez.github.io/axon/patrones.html) | Declarados, no recordados |
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
