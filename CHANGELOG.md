# Changelog

Formato de [Keep a Changelog](https://keepachangelog.com/es/1.1.0/);
versionado según [SemVer](https://semver.org/lang/es/).

El **formato del manifiesto** todavía puede cambiar de forma incompatible antes de
`1.0.0`. La superficie de comandos es estable: un comando puede ganar banderas, no
perderlas.

## [0.8.0] — 2026-09-08

### Añadido

- **El plan neutral tiene esquema publicado.** Un `axon-infra-*` recibe el plan por stdin
  y hasta ahora tenía que deducir su forma de un ejemplo: todo plugin ahí fuera estaba
  adivinando, y el día que aparece un campo nadie se entera. `axon infra --schema` emite
  el JSON Schema 2020-12, que **no nombra ningún proveedor** —el plan tampoco, y hay un
  test que lo comprueba— y en el que **cada objeto rechaza lo que no declara**, que es lo
  que lo convierte en contrato y no en documentación. Está escrito a mano, así que lo
  único que lo mantiene honesto es que un plan real pase por un validador de verdad, y
  que la deriva se atrape en los dos sentidos: un campo agregado al plan y no al esquema
  falla, y uno declarado en el esquema que el plan no tenga falla también.

## [0.7.0] — 2026-09-08

### Añadido

- **La prueba de carga entra al límite en vez de sentarse en él.** Una prueba plana en el
  número declarado contesta *aguanta lo que dijimos*; nunca ve qué pasa **un paso más
  allá**, que es el momento que decide si degrada o se cae. La rampa sale de
  `rate_limit`: la mitad, el declarado, un tramo sosteniéndolo y 25% por encima. Y hace
  la distinción que existe para hacer — **un 429 es el límite funcionando y un 5xx es el
  servicio rompiéndose**—: k6 cuenta los dos igual, así que el script lleva dos métricas
  propias y el veredicto las lee. También reporta no haber visto ni un 429 en toda la
  rampa: o el límite nunca se alcanza, o nadie lo aplica.
- **[Un comando cada uno](https://andrew-tellez.github.io/axon/tour.html)**: los 27
  comandos con un ejemplo, agrupados por cuándo se alcanzan. Un test la sujeta en los dos
  sentidos: un comando sin ejemplo es una capacidad que nadie va a encontrar, y un
  ejemplo de un comando que no existe es una página que miente.

### Corregido

- **El `rate_limit` declarado no lo aplicaba nadie.** Viajaba a `k8s` como una anotación
  para el controlador de otro y en `local` no hacía absolutamente nada: la prueba de
  carga le pasaba por encima sin un solo 429. Ahora el edge lleva el middleware, con el
  límite por minuto —la unidad en que se declara— y un burst de una décima, porque si no
  el tráfico que no es perfectamente parejo se estrangula por debajo de su propio límite.
  Gana el más estricto de las rutas que comparten router.

### Cambiado

- El demo pasó a **58 comprobaciones**, y la de carga corre contra el edge y no contra el
  servicio: pegarle directo mediría un límite que nadie impone.

## [0.6.0] — 2026-09-08

### Añadido

- **`scopes`: quién puede llamar, y no solo que haya alguien.** `auth = "required"` dice
  que quien llama está autenticado y nada más: cualquier token válido, incluido uno
  emitido para leer, puede devolver dinero. El gateway valida el token —firma, expiración,
  audiencia— y entrega lo concedido; el servicio decide si eso cubre lo que el método
  declaró, con un `requireScopes` generado que contesta `403 insufficient_scope`
  nombrando el que falta, porque un 403 sin razón es un ticket. El catálogo
  `[api] scopes` es de la plataforma y `verify` exige que todos digan lo mismo: un scope
  con un error de dedo es un 403 en producción que nadie ve en una revisión. También sale
  en el OpenAPI, por operación.
- **Retención de la bodega.** Una tabla de eventos crece para siempre, y el primer síntoma
  es la factura mientras el segundo es una consulta que se cae. `retention_days` por
  servicio, con una excepción por evento para los que alguien responde por ley. Cada
  almacén lo dice en otro lado —`TTL` en ClickHouse, `partition_expiration_days` en
  BigQuery— y **Snowflake no lo dice**: ahí sale un `TASK` que borra, porque
  `DATA_RETENTION_TIME_IN_DAYS` es Time Travel, tope 90 días, y no borra una sola fila.
  Con un `ALTER` después de cada `CREATE`, porque `IF NOT EXISTS` ignora todo cuando la
  tabla ya existe y la retención es justo lo que se declara después.

### Cambiado

- El demo pasó a **23 secciones y 57 comprobaciones**.
- Dos scripts del demo llamaban a `payments` sin credencial y ahora los rechaza. Se les
  dio el scope; no se le quitó el candado.

### Corregido

- El comentario final de la cola de Snowflake se tragaba el `;` que cierra la sentencia
  —el mismo fallo que el suite ya documentaba para un comentario al final de una columna—.
- Tres formas de SQL válido que `sqlparser 0.62` no conoce (`SET OPTIONS`, un `TASK`, el
  `TTL` de ClickHouse) salen del parseo con su razón escrita y se afirman aparte; la de
  ClickHouse se aplica contra un servidor real en el demo, que es la herramienta que
  decide.

## [0.5.0] — 2026-09-08

Las tres salieron de la misma pregunta —cómo entraría esto en un repo que ya
existe— y de contestarla usando axon contra uno de verdad.

### Añadido

- **`runtime = "job"`: algo que corre y termina.** No todo lo que hace funcionar un
  negocio escucha en un puerto. Un recálculo nocturno, un backfill, un CLI: declararlos
  como contenedor dejaba como única opción honesta no declararlos, y entonces su
  infraestructura vivía en el crontab de alguien. Cada target renderiza otra cosa —un
  `CronJob` con `concurrencyPolicy: Forbid`, un Cloud Run job con su scheduler por OAuth,
  una task de ECS sin servicio con EventBridge, y en `local` una corrida única al
  arrancar, porque una expresión cron no es un periodo—. `verify` rechaza lo que se sigue
  de "corre y termina": rutas, `min_instances`, un `schedule` sobre algo que se queda
  arriba y `@daily`, que dos de los tres proveedores no aceptan.
- **`axon import openapi`: el documento que un repo ya tiene.** Un catálogo de eventos es
  una decisión que alguien tomó; un OpenAPI casi nunca —un repo NestJS lo tiene por sus
  decoradores—. Lee rutas, parámetros, cuerpos, los tipos que se pueden distinguir y **los
  estados declarados como fallas declaradas**, que es lo que un OpenAPI tiene y un
  AsyncAPI no. Y lo que se **niega a inventar** importa igual: un `timeout_ms` sería un
  número que nadie decidió con cara de decidido, e `idempotent = true` una afirmación
  sobre código que el importador no ha visto. Salen comentados, y `verify` los exige.
- **La cadena real desde OTLP o Jaeger.** Un span *es* un envelope con otros nombres, así
  que un repo con OpenTelemetry ya tiene la cadena real sin escribir una línea. `axon
  trace` lee tres formas y detecta cuál. Con `--manifests` contesta además la mitad que
  `axon traffic` no puede ver —una llamada entre servicios no pasa por el edge— y falla
  cuando una dependencia ocurre y nadie la declara: entonces el dibujo está mal, y el
  dibujo es lo que alguien lee antes de decidir qué se puede desplegar aparte.

### Cambiado

- El demo pasó a **22 secciones y 53 comprobaciones**.

### Corregido

- Un mensaje de `verify` tenía la indentación pegada por una continuación que `cargo fmt`
  colapsó: «use /v1/... or declare······`[api] versioning`».

## [0.4.0] — 2026-09-08

### Añadido

- **`axon traffic`: quién llama a qué, leído del edge.** Con un consumidor que no usa
  axon hay una asimetría que decide qué se puede saber: lo que **pide** es observable y
  lo que **lee** de la respuesta no. Esto contesta la mitad observable —quién llama a la
  versión que se está retirando, cuántas veces y desde dónde—, dice qué rutas no reciben
  tráfico *sin afirmar que nadie las llama*, porque una llamada entre servicios no pasa
  por el edge, y nombra las rutas que nadie declara. Falla en un solo caso, que es un
  hecho y no un juicio: tráfico sobre algo que ya pasó su `sunset` declarado. Para que
  hubiera algo que leer, el edge generado ahora escribe su access log en JSON con el
  header de versión.
- **`axon pact`: el pacto de un consumidor que no usa axon.** La otra mitad, cuando ese
  consumidor ya usa Pact: su archivo *ya dice* qué campos necesita, y axon no necesita un
  broker para **leerlo**. Contesta si espera un campo que nadie devuelve —renombrado, o
  el pacto quedó viejo— y cuáles de los declarados **no** lee, que es la pregunta que
  descongela un contrato. El cuerpo de una falla se compara como RFC 7807 y no contra la
  salida del método, y un status que el método no declara sale como hallazgo sobre el
  proveedor: falla así y no lo dice.
- **`axon accept`: la línea que un repo existente puede trazar.** `verify` era
  todo-o-nada, y eso lo dejaba fuera de un código que ya existe. Los avisos de hoy se
  aceptan en `axon.accepted.json`; con el archivo presente, un aviso que no está en la
  lista falla el build —su presencia *es* el opt-in— y uno que dejó de ocurrir se reporta,
  así que la lista solo puede encoger.
- **Metabase en el target local, aprovisionado desde el manifiesto.** Una métrica
  declarada y reescrita en un dashboard son dos definiciones del mismo número. `axon
  analytics --metabase` emite la conexión y una pregunta por métrica y por embudo, cada
  una apuntando a la vista generada. El demo aprovisiona uno desde cero sin tocar la
  interfaz y compara una pregunta contra la misma vista leída de ClickHouse.

### Cambiado

- El demo pasó a **21 secciones y 51 comprobaciones**.
- La nota de diseño de los [escenarios declarados](https://andrew-tellez.github.io/axon/scenarios.html):
  por qué un escenario no puede declarar lo que espera que pase, y qué haría falta para
  construirlo.

### Corregido

- El mensaje del motor desconocido estaba a medio traducir —«no esta soportado. Motores
  nativos: postgres. Un motor different one is served by…»—. Salió de usar axon contra un
  proyecto real. Y `state = "none"` recibe su propio mensaje: es lo que alguien escribe
  para decir «esto no tiene base de datos», y mandarlo a construir un plugin es mandarlo
  a construir nada.
- El test de la política escribía un `.ts` temporal dentro del ejemplo y lo borraba; el
  typecheck que corre en paralelo lo veía aparecer y desaparecer. Solo caía a veces y
  solo en un runner.

## [0.3.0] — 2026-09-08

### Añadido

- **Fallas declarables.** `errors` en un método, declarado igual que `in` y `out`, porque
  cómo falla es parte del contrato y hasta ahora vivía en el cuerpo del handler, donde el
  llamador no lo ve. `retriable` **cambia el cliente generado**: una falla que el otro
  lado declaró final no se reintenta, porque reintentar una tarjeta rechazada termina en
  la misma respuesta y de paso gasta el presupuesto de tiempo del llamador —que en una
  saga es lo que queda para compensar—. De la misma declaración salen la tabla y un
  `fail()` tipado, el cuerpo `problem+json`, una respuesta por código en `axon openapi` y
  una suite del testkit que sujeta a las tres.
- **Versionado de endpoints, con ciclo de mantenimiento.** Dos esquemas, y `verify` exige
  que toda la plataforma declare el mismo. En `path`, `/v1` y `/v2` conviven y la vieja
  anuncia `Deprecation`, `Sunset` y el sucesor como `Link`, en los formatos que pide cada
  RFC. En `header` —el esquema de Stripe— la ruta no cambia, el llamador fija una versión
  con fecha y el servidor tiene UNA implementación más un adaptador por versión que
  cambió de forma: eso es lo que permite tener viva una versión de hace años. axon genera
  los tipos de cada forma vieja y la cadena tipada paso a paso; el mapeo de campos lo
  escribe una persona. El ciclo se declara en días —ventana mínima de soporte y ventana
  de LTS— y se puede refutar.
- **Consumo declarado.** `uses` dice qué campos lee de verdad cada consumidor, y no puede
  mentir: los campos que nadie declaró **no existen de este lado**, así que leerlos no
  compila. Con eso, quitar un campo publicado nombra a quién lo lee, y si nadie lo lee
  deja de ser un cambio incompatible. Es el valor de Pact sin grabar tráfico ni broker.
- **Reglas sobre una métrica.** `[rules.*]` declara el lazo que hoy vive en una alerta de
  dashboard más un runbook que nadie corrió: qué condición sobre qué métrica lleva a qué
  palanca. Solo **propone** —`mode = "apply"` se rechaza con su razón—, propone al entrar
  y no una vez por ventana, y excluye la ventana en curso. Las guardas son la respuesta a
  Goodhart: otra métrica que tiene que aguantar en las mismas ventanas, o no propone.
- **`axon tui`**: el sistema dibujado y animado, con la topología como grafo de resortes,
  el veredicto, las versiones y lo que cambió contra el baseline. `--frames N` lo
  renderiza a stdout, que es lo que hace el dibujo comprobable en CI.
- **`axon versions`** cuenta el ciclo de vida de la API sin bloquear, y **`axon openapi
  --api-version`** emite el documento como era en esa versión.
- **`include`**: el manifiesto de un servicio se puede partir por feature. Toda colisión
  es error nombrando los dos archivos, y la partición es invisible desde afuera.
- **`axon build` acepta una URL**: se puede generar el cliente contra lo que el otro lado
  SIRVE ahora y no contra la copia que alguien recordó commitear.

### Cambiado

- El demo pasó a **18 secciones y 46 comprobaciones** contra contenedores reales.
- Siete dependencias en el binario en vez de seis: entra `ratatui`, y se midió antes de
  aceptarla —86 → 140 crates en el árbol y 4.25 → 4.55 MB—. Está escrito en el README.
- `cog bump` mueve también la versión del binario: antes el tag decía una y
  `axon --version` otra.

### Corregido

- El test de la política escribía un `.ts` temporal dentro del ejemplo y lo borraba; el
  typecheck que corre en paralelo veía el archivo aparecer y desaparecer. Solo caía a
  veces y solo en un runner.
- `[infra]` rechaza claves que no conoce. Una clave de nivel superior escrita después de
  una tabla pertenece a esa tabla —así es TOML—, así que `include` mal puesto parseaba
  bien y no hacía nada.

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
