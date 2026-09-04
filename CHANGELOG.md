# Changelog

Formato de [Keep a Changelog](https://keepachangelog.com/es/1.1.0/);
versionado según [SemVer](https://semver.org/lang/es/).

El **formato del manifiesto** todavía puede cambiar de forma incompatible antes de
`1.0.0`. La superficie de comandos es estable: un comando puede ganar banderas, no
perderlas.

## [No liberado]

## [0.1.0] — 2026-09-04

Primera versión pública. Preview.

### Añadido

**El compilador**

- `axon build` — contratos tipados, envelope con cadena causal, clase base abstracta,
  `dispatch()` idempotente, tabla de transiciones de las máquinas de estado y clientes
  resilientes. Target nativo TypeScript.
- `axon test` — testkit autocontenido: dobles en memoria de `Bus`, `Inbox` y `Outbox`,
  fixtures derivadas del esquema del **emisor** de cada evento, y dos suites exportadas.
- `axon openapi` — OpenAPI 3.1 de toda la plataforma, con `Idempotency-Key` obligatorio
  en métodos mutantes y errores RFC 7807.
- `axon import asyncapi` — AsyncAPI 2.x y 3.x, JSON o YAML, a manifiesto. Traduce la
  semántica invertida de 2.x.

**Infraestructura**

- `axon infra` — plan neutral renderizado a `local`, `gcp`, `aws`, `k8s` o `plan`
  (JSON). Cubre el edge, la mensajería con DLQ, el cómputo, el estado, los buckets con
  su CDN y los secretos.
- `axon rls` — políticas RLS por fila y vistas enmascaradas por columna, como una
  migración más.
- `axon ci` — pipeline de GitHub Actions: los gates los sabe axon, el despliegue sale
  del `--target` y el layout del repo de `axon.policy.toml`.
- Entornos como deltas: `[env.prod]` sobrescribe `[infra]`.

**Verificación**

- `axon verify` — contratos, resiliencia, patrones de API, máquinas de estado,
  migraciones, seguridad con su mapeo OWASP, CAP y gobernanza.
- `axon baseline` — snapshot de los contratos publicados. Una versión publicada es
  inmutable, y el diff del baseline es la vía de escape para retirarla.
- `axon.policy.toml` — reglas del equipo, versionadas.
- Plugins `axon-check-*` que bloquean el pipeline igual que una regla nativa.

**Diagramas y depuración**

- `axon graph`, `classes`, `er`, `states` y `seq` — todos Mermaid, ninguno una fuente de
  verdad nueva. El ER se introspecta de las migraciones con un parser SQL.
- `axon trace` — la cadena causal real desde el `causationId` de los envelopes.
  `axon seq --events` y `axon trace --seq` son directamente comparables: ese `diff` es
  la prueba de extremo a extremo.
- `axon discover` — registro de servicios y métodos, de disco y de servicios corriendo.

**Plugins**

- Cualquier ejecutable `axon-*` en el `PATH`: `axon-gen-<lang>`,
  `axon-infra-<target>`, `axon-check-<regla>`. Sin ABI y sin cargar librerías.
- `plugins/axon-gen-go` — generador de referencia escrito en Go.

**Patrones impuestos por generación**

Transactional outbox, consumidor idempotente, cadena causal, dead letter, database per
service, circuit breaker con backoff y jitter, `Idempotency-Key`, expand/migrate/contract,
state pattern y el lado del teorema CAP declarado.

### Notas

- Un solo target nativo de código (TypeScript), un solo modelo de ejecución
  (`container`) y un solo dialecto SQL (PostgreSQL). Todo lo demás entra por plugin.
- `verify` compara declaraciones entre sí y contra las migraciones; **todavía no
  compara contra el cloud desplegado**.
- El `rate_limit` del edge se emite como anotación en `k8s`: aplicarlo depende del
  controlador que la lea.

[No liberado]: https://github.com/Andrew-Tellez/axon/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/Andrew-Tellez/axon/releases/tag/v0.1.0
