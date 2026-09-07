# Changelog

Formato de [Keep a Changelog](https://keepachangelog.com/es/1.1.0/);
versionado según [SemVer](https://semver.org/lang/es/).

El **formato del manifiesto** todavía puede cambiar de forma incompatible antes de
`1.0.0`. La superficie de comandos es estable: un comando puede ganar banderas, no
perderlas.

## [0.2.0] — 2026-09-07

### Añadido

- **Sagas declarables.** `[saga.<n>]` declara los pasos y su compensación; axon genera el
  coordinador completo —avance, compensación en orden inverso, presupuesto de tiempo— y el
  barrido que retoma la saga que quedó colgada, desplegado en los cuatro targets. El demo
  lo mide contra contenedores: compensa, retoma desde el journal y no vuelve a barrer una
  saga cerrada.
- **Event sourcing y CQRS declarables.** El flujo como fuente de verdad con `UNIQUE
  (stream_id, version)`, el `fold` que rechaza huecos, fotos con versión de reglas —caché,
  no verdad— y su borrado, punto de control por flujo, y reconstrucción sobre una tabla
  sombra para que nadie lea una vista a medias. Con el relay como único que publica, y la
  regla que lo exige.
- **Bodega y BI.** Una tabla por evento y los embudos derivados de la cadena causal
  declarada, en tres dialectos —BigQuery, Snowflake y ClickHouse—, cada uno validado con
  su propio parser. Camino de ingesta en `local` y en k8s con Vector, detección de drift
  contra `information_schema`, y el PII excluido o hasheado según se declare.
- **Métricas de negocio declarables.** `[metrics.<n>]` baja a una vista en la bodega junto
  a los embudos. Lo que hace que valga declararlas es lo que `verify` refuta: una métrica
  sobre un evento que nadie emite, una suma sobre algo que no es número, una dimensión que
  el evento no declara y una dimensión que es un campo personal.
- **Fallas declarables.** `errors` en un método, declarado igual que `in` y `out`, porque
  cómo falla un método es parte de su contrato. `retriable` **cambia el cliente generado**:
  una falla que el otro lado declaró final no se reintenta. De la misma declaración salen
  la tabla y un `fail()` tipado, el cuerpo `problem+json`, una respuesta por código en
  `axon openapi`, y una suite del testkit que sujeta a las tres.
- **pgdog desde el manifiesto**, con las reglas que lo hacen seguro, levantado en el target
  `local` y con el aislamiento por tenant medido: 20 de 20 conexiones vieron solo lo suyo.
- **Escalado, alta disponibilidad y pruebas de carga medibles**: `pool_size`,
  `max_connections`, réplicas, standby, backups y PITR, con `axon load` y sus umbrales.
- **Feature flags con OpenFeature**, servidos por flagd, con `owner`, caducidad, rollout
  pegajoso y las reglas que nadie más impone.
- **`axon cap`**: no bloquea, explica las consecuencias de la combinación que declaraste.
- **El registro, desde lo que está CORRIENDO.** Cada servicio sirve su manifiesto en
  `/.well-known/axon.json` y `axon discover` lo cruza con el repo.
- **Documentación versionada** en mdBook, con cada bloque ```toml pasando por `axon
  verify`, la salida citada buscada en el código que la imprime, y dos páginas nuevas: la
  arquitectura en diagramas y el demo medido.
- **OpenTelemetry en los cuatro targets.** axon no trae un SDK ni inventa un formato:
  el `traceparent` del envelope ya es el contexto W3C que propaga OTel. Lo que aporta
  es levantar el backend en `local` (Jaeger) e inyectar las variables estándar en los
  cuatro targets, con los atributos de recurso derivados del manifiesto (`owner`,
  `tier`, `version`) y el muestreo derivado del `tier` — tier 0 se traza entero, y en
  `local` se traza todo sin importar el tier.
- `demo.sh` verifica la forma del árbol de spans en CI: un solo raíz, cero huérfanos y
  la traza cruzando los dos servicios.

### Cambiado

- **Todo lo que se lee está en inglés**: mensajes, identificadores y comentarios del
  compilador, los tests, el ejemplo y el libro. Los commits y este changelog siguen en
  español, que es la lengua en la que se decide.
- El demo pasó de 6 a **16 secciones y 39 comprobaciones** contra contenedores reales, y
  se diagnostica solo cuando falla en CI.

### Corregido

- El cron golpeaba una ruta que nadie servía: el código generado servía `/sweep` y
  `/prune` mientras la infraestructura seguía emitiendo los nombres viejos. El `curl ...
  || true` se comía el 404, así que el barrido de sagas y el borrado de fotos llevaban
  tiempo sin correr sin que nada avisara. Ahora la ruta sale de una sola función y un test
  la compara en los dos sentidos.
- El outbox recibía su propia conexión en vez de la transacción de quien llama, que es
  exactamente el dual-write que el outbox existe para evitar: si la transacción se
  revierte, el cambio de estado no ocurre y el evento sí.
- El chequeo de drift de la bodega borraba el histórico al restaurar, y el cargador casaba
  columnas por posición: al volver una columna al final, el importe se guardaba en la
  moneda y la métrica contestaba NULL sin error. Los dos se encontraron corriendo el demo
  dos veces seguidas.
- El demo pasaba en macOS y fallaba en Linux: ClickHouse hacía `chown` de todo lo montado
  en su `user_files`, y con `.axon` ahí toda escritura posterior del host fallaba. CI
  llevaba unos 35 commits en rojo por eso y por tres causas más.
- El envelope generado fijaba los flags del `traceparent` en `01` en vez de heredarlos
  de su causa. Declarar «muestreado» sobre una traza que no lo está deja fragmentos
  colgando de un padre que nunca se exportó, y en la UI se ve como varias trazas
  cortas en vez de una.
- `pages` no podía desplegar desde un tag: el entorno `github-pages` solo admite la
  rama por defecto. Ahora se encadena al release con `workflow_run`.
- `release` quedaba en cola para siempre: los runners `macos-13` están retirados. El
  binario de macOS Intel se compila cruzado desde el runner arm64.
- Un test valida que los workflows del propio repo parseen: antes solo se validaba el
  YAML que axon genera.

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
