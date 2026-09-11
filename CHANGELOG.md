# Changelog

Formato de [Keep a Changelog](https://keepachangelog.com/es/1.1.0/);
versionado según [SemVer](https://semver.org/lang/es/).

El **formato del manifiesto** todavía puede cambiar de forma incompatible antes de
`1.0.0`. La superficie de comandos es estable: un comando puede ganar banderas, no
perderlas.

## [0.30.0] — 2026-09-11

### Añadido

- **`axon lsp`: lo que `verify` ya sabe, dentro del editor.** Los manifiestos son TOML,
  así que el editor ya los pinta; lo que no puede ver es la mitad que vive entre archivos
  —un evento que nadie consume, una garantía que la topología contradice—, y eso es
  justo el informe de `verify`. El servidor relee el workspace al abrir y al guardar y
  publica cada hallazgo como diagnóstico, colocado en la línea que lo causó: un error de
  parseo trae su propia línea, y el resto se ubica por lo que el mensaje ya nombra entre
  comillas invertidas. Completa también: las claves del bloque donde está el cursor y la
  lista cerrada de valores que acepta una clave, ambas sacadas del modelo mismo —un
  campo que se renombra se renombra en el editor— y de las mismas constantes que usan las
  reglas. Y salta: del evento consumido al `[emits]` que lo declara, del `[[depends]]` al
  bloque del método del otro lado. Y al pasar el cursor por un evento contesta lo que su
  archivo no puede: quién lo emite, quién más lo lee —nadie, si nadie— y cuáles de sus
  campos son datos personales. Lista referencias, que es la pregunta de antes de retirar
  un método y la que un grep contesta mal, porque `getOrder` también casa con
  `getOrderV2`. Sobre stdio, sin dependencias nuevas.

### Cambiado

- **Una clave que el modelo no conoce se rechaza en vez de ignorarse.** `transport =
  "nats"` arriba de un manifiesto parecía declarado, se leía como una decisión en una
  revisión, y no hacía nada: el modelo nunca tuvo ese campo, y el cliente generado dice
  en su propio comentario que el framework no elige transporte. Ignorar es el peor de los
  tres finales posibles. Ahora el error lista las claves que sí existen. El precio es que
  un manifiesto escrito para un axon nuevo falla en uno viejo, que es el trato que hace
  cualquier esquema y un fallo mejor que el silencioso. **Es incompatible**: un manifiesto
  con una clave muerta deja de cargar hasta que se borre. Las de los ejemplos —`transport`
  y `discovered_from`— ya se fueron; la segunda era procedencia de verdad y quedó como
  comentario, que es donde vivía su valor.

## [0.29.0] — 2026-09-11

Tres reglas que salieron de mutar los manifiestos del ejemplo y del banco de pruebas, una
por una, y ver qué pasaba limpio. De sesenta y dos mutaciones, cincuenta y nueve ya las
cazaba `verify`; estas tres son las que no.

### Añadido

- **Un presupuesto de cero no es un presupuesto.** Faltar `timeout_ms` ya era error; un
  `timeout_ms = 0` pasaba limpio, y no es «sin límite»: es un límite en el que no cabe
  nada. El cliente generado corre la llamada contra un `setTimeout(.., 0)`, que dispara
  en el tick siguiente, así que **toda** llamada termina en `TimedOut` antes de que la
  petición salga. Typechequea, despliega, y la dependencia se ve caída. Es error en los
  dos lados: en la llamada, donde el cliente corre el reloj, y en el método, porque lo
  que promete es contra lo que presupuestan sus llamantes.
- **Un cero de retención de backups no es una ausencia.** Había regla para un `tier = "0"`
  sin `backup_retention_days` y para menos de siete días en un tier 0. Un
  `backup_retention_days = 0` en cualquier otro tier era silencio, y no es lo mismo que no
  declararlo: ausente toma el default de la plataforma, un cero es alguien que lo
  escribió. Warning y no error —una base de scratch tiene derecho a no tener backups—,
  pero dicho una vez, porque lo que no tiene derecho a pasar es que nadie se entere el día
  que importa.

### Seguridad

- **Un juego de claves servido en `http://` es una verificación contra las claves de
  otro.** `[auth]` ya rechazaba `none`, un HMAC donde hay claves publicadas, una
  revocación que el mecanismo no puede cumplir y un tenant que no viene del token. No
  miraba de **dónde** salen las claves. Sobre http en claro, quien esté en el camino sirve
  su propio juego y desde ahí acuña tokens que el servicio acepta: la firma cuadra, contra
  el juego equivocado, y nada se ve roto mientras pasa. Lo mismo con el
  `introspection_url`, cuya *respuesta* es la decisión. `localhost` es la excepción que el
  target local necesita y no está en el camino de nadie.

### Corregido

- **El `demo.sh` del ejemplo volvió a ser re-ejecutable.** `check-rules.sh` borraba solo
  sus propias filas sembradas y `check-apply.py` siembra con otra etiqueta. Una corrida
  que muere a la mitad dejaba esas filas, y la siguiente las sumaba en los mismos días:
  los importes al doble, una caída que deja de parecer una caída, y el fallo acusando a la
  regla en vez de al residuo. Dejaba de ser re-ejecutable justo después de fallar, que es
  cuando más falta hace.

### Pruebas

- **El esquema del warehouse se aplica a un ClickHouse de verdad.** Era el último
  generador que se verificaba con un grep, y el que ya había mostrado lo que un grep no
  ve. Se comprueba lo que una persona hace con el archivo: sustituir el dataset, aplicarlo
  y **consultar las tablas por nombre** —lo que un identificador con punto rompe—, que
  ninguna caiga en `default`, y que el embudo y la métrica declarados existan como vistas.

## [0.28.1] — 2026-09-11

### Corregido

- **Sin fuentes, el directorio actual.** Leer nada era una respuesta, y la peor: `axon
  verify` sin argumento encontraba `axon.baseline.json` —que vive en ese mismo
  directorio— y concluía que habían borrado todos los eventos y métodos publicados. Diez
  errores y exit 1 sobre un repo donde no pasa nada. `graph` contestaba `graph LR` a
  secas, `discover` contestaba `{}` y `pact` decía que el proveedor no tiene manifiesto:
  cuatro veredictos sobre el proyecto, cuando lo que pasaba es que nadie dijo dónde
  mirar. La mitad del comando ya defaulteaba a `.` —de ahí salía el baseline— y la otra
  mitad no. `build` y `test` no cambian: ahí una lista vacía significa «sin pares».
- **Dos servicios con una tabla del mismo nombre son dos tablas en `axon er`.** Una base
  por servicio significa que `orders` y `notifier` tienen cada uno su `inbox_seen`, y
  nombradas por la tabla sola son UNA entidad para mermaid: las une y dibuja las columnas
  dos veces, así que el dibujo describe un esquema que nadie tiene. El id lleva el
  servicio —`ORDERS_INBOX_SEEN`— y el alias mantiene el nombre legible,
  `["orders.inbox_seen"]`, que es además lo único que dice en qué base vive cada tabla:
  el `%% service:` que ya estaba es un comentario, y un comentario no se renderiza.
- **El `demo.sh` del ejemplo, verde otra vez.** El arreglo del dataset de 0.28.0 dejó el
  esquema como `"@dataset"."tabla"` y tres scripts lo sustituían con un `sed` que
  matcheaba la forma vieja. El `sed` dejó de sustituir en silencio, el `@dataset`
  sobrevivió y ClickHouse contestó con un error de sintaxis que no nombra lo que se
  rompió; ahora hay un guardia en los tres que falla ahí mismo.

### Pruebas

- **Los cinco diagramas pasan por mermaid, la OpenAPI por `redocly lint --extends=spec`,
  los 33 recursos de `--target k8s` por kubeconform con los esquemas de sus CRDs, y las
  banderas por el esquema publicado de flagd.** Eran los cuatro generadores que se
  verificaban contra su propio golden, y un golden dice que el texto no cambió, no que el
  texto sirva: `axon graph` vivió una versión entera muriendo al parsear y la OpenAPI
  salió con 22 errores de spec, con todos los asserts en verde. `tests/js/mermaid.mjs` y
  `tests/js/flagd.mjs` son los mismos archivos que se corren a mano antes de pegar algo
  en cualquier lado.

## [0.28.0] — 2026-09-10

### Añadido

- **`axon verify` exige las dos tablas que axon nombra por su cuenta.** `outbox` e
  `inbox_seen` no son una convención que alguien pueda renombrar: la regla de inquilino
  las exime por ese nombre exacto, el pooler reserva conexiones para el relevo que drena
  `outbox`, y `rls` las deja fuera de las políticas. Aun así, declarar `[patterns] outbox
  = true` y no escribir la migración pasaba limpio. Es la única combinación que aplica sin
  un error y revienta en el primer insert, en el camino cuya razón de existir es que
  ningún evento se pierda; con `[consumes.*]` y sin `inbox_seen` es peor, porque el broker
  entrega al menos una vez y el handler corre de nuevo en cada reentrega sin nada que diga
  que ya pasó. Un servicio sin `[infra] state` no tiene migraciones que mirar y no se le
  pide ninguna de las dos. La plantilla de `axon init` lo prometía desde la versión
  anterior; ahora es cierto.
- **El dibujo de `axon tui` dice qué significan sus propias etiquetas.** `[AP]` junto a un
  nodo se lee como un estado —algo arriba, algo sano— cuando es una decisión sobre qué
  hace el servicio durante una partición. Va una leyenda en el borde inferior del grafo, y
  en el panel, junto a cada servicio, los dos campos declarados en palabras: `AP ·
  eventual ≤3000ms · degrades`. Cada mitad sale de un campo distinto a propósito, porque
  el par puede ser contradictorio y una frase construida de ambos se leería como absurdo
  en vez de mostrarlo.

### Corregido

- **Todo esquema de warehouse creaba las tablas en la base equivocada.** `axon analytics`
  entrecomillaba el par entero —`"dataset.tabla"`—, que es UN identificador que por
  casualidad tiene un punto. En BigQuery la forma con backticks significa las dos cosas,
  así que se leía bien mientras nadie lo aplicara en otro lado. Medido contra un ClickHouse
  24: `CREATE TABLE "bench.demo"` aterriza en `default` bajo el nombre literal
  `bench.demo`, y `SELECT FROM bench.demo` contesta `UNKNOWN_TABLE`. El esquema aplica
  limpio y el dataset al que apuntaba queda vacío, que es exactamente el desenlace que
  `[analytics] warehouse` existe para evitar. Los once puntos de llamada pasan por
  `qualify`, que entrecomilla cada mitad por separado.
- **`axon graph` moría al parsear en las líneas que nombran los eventos.** Un evento se
  llama `order.placed@v1`, y mermaid 11 estrenó ids de arista y metadatos de nodo con `@`
  (`e1@-->`, `A@{...}`): el `@` sin comillas dejó de ser texto. Va entrecomillado todo el
  texto libre —etiquetas de arista y el nombre de cada nodo—, que de paso deja pasar un
  punto, una llave o una barra. Los otros cuatro diagramas ya parseaban.
- **La OpenAPI no declaraba los parámetros de sus propias rutas.** Cada ruta con plantilla
  salía sin un solo `parameters`. No es un documento incompleto: el spec exige un objeto
  por plantilla, cualquier validador lo rechaza —22 errores de `redocly lint` sobre un
  repo de cinco servicios— y un cliente generado se queda con un método cuyo id no tiene
  dónde ir. El tipo sale de `in`, que es donde el placeholder ya estaba declarado.
- **`axon import openapi` escribía un manifiesto que no parsea.** `{tenantId}` es
  parámetro de ruta Y propiedad del cuerpo: así se ve un documento bien formado. El
  importador los sumaba sin más y escribía una clave duplicada, que no es TOML. Para axon
  es un campo solo —`in` dice qué necesita el método, no cómo viaja— y gana el primero. Es
  la mitad opuesta del arreglo anterior: las dos se separan solas, así que ahora hay un
  test del viaje redondo en vez de uno por mitad.
- **Lo que la herramienta genera habla un solo idioma.** Un diagrama de secuencia que
  contestaba `respuesta`, un ER comentado `%% servicio:`, una NetworkPolicy que decía
  `# nadie` y un warehouse vacío que explicaba `Ningun servicio exporta eventos`. El
  código piensa en español y eso es asunto de este repo; el archivo que lee otra persona
  no.

## [0.27.0] — 2026-09-10

### Añadido

- **El panel de servicios de `axon tui` dice qué es cada servicio, no cuánto tiene.**
  Contaba `5 methods` y nombraba `postgres`: con un servicio alcanza, con cinco no dice
  nada, y lo que alguien mira una topología para saber es qué puerta está abierta. Ahora
  es un bloque por servicio: la base con la columna de inquilino, el shard, las réplicas y
  el pool; la caché y el índice con de qué son; los buckets; el cron de un
  `runtime = "job"`, que antes no aparecía en ninguna parte; cada topic que emite **con
  quién está del otro lado** —incluidas la view y el aggregate del mismo servicio, que son
  consumidores que ninguna arista dibuja—; cada suscripción con el handler al que entra;
  una línea por ruta expuesta, marcada con su `sunset` si se está muriendo; a quién llama
  con su presupuesto, **quién lo llama**, los pactos que hay contra él y sus banderas.
- **Un panel nuevo, `contracts`: la relación que existe ENTRE los servicios.** Cada evento
  con cuáles de sus campos lee cada consumidor, cada llamada con su timeout, sus reintentos
  y qué lee de la respuesta, y los pactos de `pacts/` de quien no tiene manifiesto. Es lo
  único que un cambio puede romper en el repo de otro, y era lo único que no se podía leer
  en un solo lugar.
- **`--frames N` recorre los paneles, uno por cuadro.** Renderizar siempre el primero
  dejaba los otros cuatro como una proyección que nadie puede verificar, que es justo lo
  que esa bandera existe para evitar: `--frames 5` es la TUI entera como texto. Y el panel
  crece hasta media pantalla en vez de quedarse en seis filas fijas.

### Corregido

- **`axon init` abortaba a la mitad y dejaba el proyecto escrito por partes.** Sobre un
  directorio que ya tenía un proyecto, escribía el manifiesto, `services/<svc>/` y la
  migración y *luego* se negaba al llegar a `.env.local`. La negativa era correcta —`init`
  escribe un proyecto desde cero y nunca encima de otro— pero llegaba tarde, y un proyecto
  escrito por mitades es peor que uno no escrito. Ahora lista los siete archivos, comprueba
  los siete y sólo entonces escribe: la negativa dice que no se escribió nada.

## [0.26.1] — 2026-09-10

### Corregido

- **Los healthchecks del target local no esperaban lo que tarda arrancar.** Compose da tres
  intentos por defecto, y con `interval: 2s` eso son seis segundos: menos de lo que tarda
  `initdb` más el reinicio que hace Postgres después en un runner frío. Una base que arrancó
  perfecto se declaraba unhealthy y todo lo que dependía de ella nunca arrancaba, con un log
  que dice `is unhealthy` de un contenedor cuyo propio log dice `ready to accept connections`.
  Ahora postgres, el broker, trace, objetos y la caché declaran `retries: 30`, como ya hacían
  el pooler, los servicios, search y el warehouse.

## [0.26.0] — 2026-09-10

### Añadido

- **`axon tui` dice qué declara cada servicio, no sólo con quién habla.** El dibujo sólo
  tiene aristas, así que un evento que nadie consume todavía —o una base de datos, o una
  caché— no aparecía en ninguna parte, y un proyecto de un solo servicio salía como un nodo
  suelto sin nada alrededor. El panel `services` es ahora el que abre: una línea por
  servicio con lo que emite, lo que consume, cuántos métodos tiene y sobre qué corre.

### Corregido

- **Los paneles truncaban en silencio.** Se quedaban con las tres o cuatro primeras líneas
  y cortaban cada una al ancho, sin decir que estaban escondiendo algo. Ahora el texto se
  envuelve, `↑↓` (o `j`/`k`) lo recorren, `[tab]` resetea el offset al cambiar de panel y
  el `1/3 ↑↓` de la esquina sólo aparece cuando hay algo debajo.

## [0.25.2] — 2026-09-10

### Corregido

- **El dibujo de `axon tui` se iba a la esquina cuando un eje se colapsaba** —un solo
  servicio, o varios alineados—: el span se forzaba a `0.001` en vez de tratarse como lo
  que es, y todos los nodos caían sobre la arista baja. Un eje colapsado ahora se centra.
- **`--frames` no renderizaba lo que ve una persona**: daba cuatro pasos de simulación por
  frame contra el uno del bucle vivo, así que la animación corría a 4× en la única salida
  que un test puede mirar, y `--frames 1` imprimía `frame 4`.
- **`[tab]` acumulaba sin cota** y el módulo que lo salvaba vivía en el índice, lejos de la
  tecla. El número de paneles es ahora una constante que comparten el array y la tecla que
  lo recorre.

## [0.25.1] — 2026-09-09

### Corregido

- **El `.gitignore` que escribe `axon init` no cubría `.env.local`**, que es el único archivo
  de ese layout cuyo trabajo entero es quedarse fuera de git: ahí van los valores de los
  secretos que el manifiesto declara. Salió rehaciendo prueba-axon desde cero, en el primer
  `git add -A`.

## [0.25.0] — 2026-09-09

### Añadido

- **`[ci] contracts_path` acepta una lista.** Un repo que guarda el contrato en dos
  lenguajes tenía uno solo en el gate; el otro no se comparaba contra nada y se quedaba
  viejo sin que nadie se enterara. Cada ruta se regenera en el lenguaje que nombra su
  extensión. La forma de una sola cadena sigue siendo la de siempre.

### Corregido

- **La ruta de migraciones en el pipeline se salía del checkout.** `migrations` es relativa
  al manifiesto —así se lee en todos lados— y el pipeline corre desde la raíz del repo, así
  que un `"../sql/inventory"` se emitía tal cual: `filesystem:./../sql/inventory`. Ahora se
  resuelve contra `manifests_dir` antes de escribirla.

## [0.24.3] — 2026-09-09

### Corregido

- **El gate de contratos del pipeline regeneraba siempre TypeScript.** `axon ci` emite un paso
  que regenera el codigo y falla si hay diff; pedia `--lang ts` sin mirar qué guarda el repo,
  así que un servicio en Go se comparaba contra un archivo que no usa — un gate que pasa
  siempre. Ahora el lenguaje sale de la extensión de `contracts_path`, en GitHub y en GitLab.

## [0.24.2] — 2026-09-09

### Corregido

- **El generador de Go emitía código que no compila** cuando un manifiesto declaraba
  `scopes` y ningún `errors`: `RequireScopes` devuelve un `&Problem` y el tipo solo se
  escribía al lado de las fallas declaradas — `undefined: Problem` en la primera
  compilación. Ahora `Problem` sale si hay fallas **o** scopes, y el conformance genera ese
  caso exacto y lo pasa por `go build`.

## [0.24.1] — 2026-09-09

### Corregido

- **El andamio contestaba 404** a la ruta que el manifiesto declara, sin decir por qué. Ahora
  contesta **501** con `problem+json` nombrando el método y el archivo: *«the manifest
  declares GET /v1/things/{thingId}; implement it in services/pagos/index.ts»*. Un stub que
  contestara 200 con datos inventados sería justo lo que este proyecto existe para evitar.
- **Los pasos siguientes decían `docker compose up -d --wait`**, que después de cambiar el
  código vuelve a correr la imagen vieja sin decir nada — lo vi depurando por qué mi propio
  stub no aparecía. Ahora dicen `--build`.

## [0.24.0] — 2026-09-09

### Añadido

- **`axon init <servicio>`: la causa, no el mensaje.** En 0.23.0 arreglé lo que los cinco
  hallazgos *decían*. Tres tenían causa, y era la misma: la distribución del proyecto. `init`
  escribe uno que verifica limpio y **arranca** —el manifiesto en la raíz, para que
  `migrations` no necesite `../`; su migración; el Dockerfile que el compose construye; la
  policy; el `.env.local`—. Comprobado: cuatro contenedores sanos y el edge contestando. Se
  niega a escribir sobre un proyecto existente.

- **Las dos convenciones de migración.** axon aceptaba una y le decía a todo repo que ya
  usaba la de Flyway que no tenía prefijo numérico, sobre un archivo llamado `V1__x.sql`.
  Ahora se aceptan `001_<nombre>.sql` y `V1__<nombre>.sql`, y **las banderas que el pipeline
  le pasa a Flyway salen de la que el repo usa**. Lo que se niega es mezclarlas: en un
  directorio no tienen orden definido, y Flyway solo ve las que casan con el prefijo que le
  dieron — la mitad no se aplicaría nunca.

### Corregido

- **El orden de las migraciones se lee de la versión, no del nombre.** `V10__` va antes que
  `V2__` como texto: la décima correría segunda, y el fallo sería una columna que todavía no
  existe.
- **El puerto del broker era fijo**, así que dos proyectos axon en una máquina chocaban con
  un error que se lee como «el broker está roto» y es «algo ya tiene el 4222». Hay una prueba
  que recorre el compose entero exigiendo que **todo** puerto se pueda mover.

### Cambiado

- La suite pasó a **106 pruebas**.

## [0.23.0] — 2026-09-09

### Corregido

Cinco mensajes que tenían razón y no servían, encontrados **usando la CLI en un proyecto
vacío** — que es la única forma de encontrarlos: quien conoce el repo de ejemplos no ve
ninguno.

- **`migrations` se resuelve desde el directorio del manifiesto**, así que la distribución
  más natural —`manifests/` al lado de `sql/`— no lee ninguna. Con un `[crud.*]` eso era un
  aviso, o sea que la mitad de lo que se compra declarando un CRUD no estaba pasando, **en
  silencio**. Ahora es error, y dice desde dónde miró y que un layout con `manifests/`
  necesita `../sql/...`.
- **«no numeric prefix» sobre un archivo llamado `V1__product.sql`** se lee como un fallo de
  axon. Ahora nombra la forma que espera (`001_<nombre>.sql`), de dónde sale, y que las dos
  convenciones no se pueden mezclar en un directorio.
- **El compose construye `services/<svc>/Dockerfile`** y sin nada ahí docker falla con un
  `lstat` que no nombra nada. axon escribió esa ruta, así que ahora `verify` lo avisa.
- **Ese compose moría por un `.env.local`** que un proyecto sin secretos declarados no tiene
  por qué tener: ahora va con `required: false`.
- **Un manifiesto que no menciona analytics** recibía un error sobre un almacén que nunca
  pidió. El rechazo está bien —las tablas se quedarían vacías— pero ahora dice que exportar
  viene **activado por defecto**, que es la parte que faltaba.

La prueba nueva reproduce el proyecto desde cero y comprueba los cinco. La suite: **104**.

## [0.22.0] — 2026-09-09

### Añadido

- **`[search]`: el índice, con lo que lo vuelve mentira.** Misma forma que la caché y reglas
  más duras, porque el fallo es peor: una caché vieja sirve **una** respuesta equivocada a
  quien pidió esa llave; un índice viejo o sin filtro **lista** filas —de otro inquilino, o
  filas que ya no existen— y nadie las pidió por su nombre, así que nada en la respuesta se
  ve mal.

  Se niega sin el inquilino en `filter_by`, si la llave no es un campo que el evento traiga
  —el reindex no podría decir qué documento cambió—, si nada lo reindexa, si un campo no es
  columna, y si se indexa un campo `pii` **sin nombrarlo**: buscar un cliente por su correo
  es una necesidad real, así que no se prohíbe; se nombra en `pii_indexed`, como
  `tenant_exempt` nombra una tabla, y entonces es una decisión que alguien tomó.

  El filtro va **en la firma**: una consulta sin inquilino no compila. Devuelve **ids** — un
  índice que además sirve el contenido es una segunda fuente de verdad que contradice a la
  fila el día que se atrasa. Y el reindex carga desde la fila, no construye el documento
  desde el evento; si la fila ya no está, el documento se borra.

  Se levanta en `local` y en `k8s`. En `gcp` y `aws` **se niega**: no hay Meilisearch
  gestionado, y emitir un dominio de OpenSearch sería axon eligiendo otro lenguaje de
  consulta a espaldas del manifiesto.

### Cambiado

- La suite pasó a **103 pruebas**.

## [0.21.0] — 2026-09-09

### Añadido

- **`[crud.*]`: los cinco endpoints sin lógica.** Crear, leer, actualizar, borrar y listar
  — lo que todo servicio reescribe y no tiene decisión dentro. A mano son cinco rutas, cinco
  scopes, cinco entradas en el OpenAPI y cinco ocasiones de olvidar el inquilino en el
  `WHERE`.

  Se expanden a `[methods.*]` **normales antes de que nadie lea el manifiesto**, así que
  `verify`, el OpenAPI, el testkit, el edge y el cliente generado funcionan sin maquinaria
  nueva. Y por eso heredan todas las reglas que ya existen — dos de las cuales cambiaron el
  diseño en cuanto las corrí: el `create` toma la **llave del llamante** (axon se niega a una
  mutación no idempotente, y un id generado en el servidor es lo que hace ese fallo
  imposible de arreglar) y el `list` **pagina por cursor**.

  Lo que hace que valga la pena declararlo es que el compilador ya lee las migraciones con un
  parser SQL de verdad: se niega si la tabla no está en ninguna migración, si un campo no es
  columna (`money` son dos), si la llave no la cubre un `PRIMARY KEY` o `UNIQUE` —la lectura
  devolvería *una de* varias filas y el update escribiría en *todas*—, si la tabla no lleva
  el `tenant_column` sin estar exenta, o si leer y escribir comparten scope.

- **`axon crud <manifiesto> --expand`**: imprime lo que un `[crud.*]` genera, como TOML listo
  para pegar. El override es uno solo —se declara el método a mano y gana **entero**—, y en
  un endpoint sobrescrito imprime lo que *habría* generado diciéndolo. Sin override parcial a
  propósito: dos declaraciones del mismo endpoint con reglas de mezcla es una pregunta que
  nadie puede contestar a las tres de la mañana.

### Cambiado

- La suite pasó a **102 pruebas**.

## [0.20.0] — 2026-09-09

### Añadido

- **`axon auth <manifiesto>`: el verificador, emitido desde el bloque.** JOSE estándar, y
  todo lo que decide si un token se acepta sale del manifiesto —los issuers, el juego de
  claves, la lista cerrada de algoritmos, la edad máxima y el nombre de cada claim—. El
  mismo archivo sirve contra el plugin JWT de better-auth, Auth0, Keycloak o Cognito
  cambiando el **manifiesto** y no el código; hay una prueba que lo mide con la forma de
  Keycloak (otro issuer, `realm_roles`, RS256).

  Pasarle la lista de algoritmos a la librería no es un detalle: sin ella la librería se cree
  la cabecera del propio token sobre cómo verificar el token, que es por donde entran `none`
  y el truco del HMAC sobre una clave publicada.

  Emitido y no enlazado: un archivo generado que alguien puede leer y editar es mejor que una
  dependencia que esconde de qué claim se fio. Y se niega donde tendría que adivinar —
  `introspection` es la API del emisor y su credencial, y `adapter` es que traes el tuyo.

### Notas

- El servidor MCP de better-auth estuvo caído durante la investigación: **5 de las 8 áreas
  quedaron bloqueadas**, incluida la de JWT/JWKS. El verificador se apoya en los RFC (7515,
  7517, 7519, 8693) y no en afirmaciones sobre un proveedor que no se pudo leer.
- La suite pasó a **100 pruebas**.

## [0.19.0] — 2026-09-09

### Añadido

- **`[auth]`: la forma del token, sin el nombre de ningún proveedor.** axon no autentica a
  nadie ni guarda una credencial. Lo que se declara es la **forma** que tiene que tener un
  token verificado, para que el `auth` del edge, los `scopes` del método y el RLS que el
  compilador ya genera dejen de ser tres esperanzas independientes que casualmente coinciden.

  Quien emite el token —better-auth, Auth0, Keycloak, Cognito, treinta líneas de `jose`— es
  un **adaptador**, igual que `Bus`, `Cache` u `Outbox`. Los nombres de claim son toda la
  superficie específica de proveedor, y son datos.

  Se niega solo lo que no puede ser una configuración legítima: `none` o un `HS*` en
  `algorithms` (los dos fallan **abierto** y parecen un 200 normal), `revocation =
  "immediate"` sobre verificación offline, `eventual` sin `max_token_age_s`, `tenant_column`
  sin `tenant_claim` —el inquilino vendría de la petición, que es el llamante eligiendo qué
  filas lee—, dos claims con el mismo nombre, `jwks` sin `jwks_uri`, y dos servicios leyendo
  el mismo token de sitios distintos.

- **`roles` y `plans` por endpoint.** El **requisito** es contrato y viaja en el OpenAPI y
  en el guard generado; el **mapeo** de rol a scopes no se declara: es configuración mutable
  del proveedor, y una segunda copia aquí se quedaría vieja en silencio. Los nombres se
  comprueban contra `[catalog.role]` y `[catalog.plan]`.

- **Impersonación**, porque decide dos cosas que el compilador ya razona: a qué inquilino se
  ata el RLS y si la escritura dice quién lo hizo de verdad. `audit = false` se niega. El
  `AuthContext` separa `subject` de `actor`, y `withTenant(ctx, tx)` es lo **único** que
  emite `SET LOCAL axon.tenant` — sin sobrecarga que acepte un string suelto.

### Notas

- Lo que a propósito **no** se comprueba es la mitad del trabajo: un `audience` único por
  servicio rompe Auth0 y Cognito, y un `issuer` escalar rompe cualquier migración de IdP.
  Una regla que dispara sobre una configuración correcta silencia a toda la familia.
- El diseño salió de un workflow de 12 agentes sobre la documentación de better-auth y de
  tres críticas adversarias que recortaron la mitad de lo propuesto. La suite pasó a **99
  pruebas**.

## [0.18.0] — 2026-09-09

### Añadido

- **`[catalog.*]`: la lista declarada, en la tabla y en el tipo.** Monedas, estados,
  motivos, países. La lista que nadie cree que valga la pena declarar, y que acaba escrita
  tres veces —un enum en un servicio, un `CHECK` en una migración y un desplegable en el
  front—: el día que alguien añade un valor, dos de las tres no se enteran.

  ```toml
  [catalog.currency]
  key = "code"
  fields = { code = "string", name = "string", decimals = "int" }
  entries = [{ code = "MXN", name = "Peso mexicano", decimals = 2 }]
  ```

  `axon catalog` emite la tabla y su semilla como **migración repetible**, y `axon build`
  el tipo unión —donde un valor fuera de la lista no compila—, la tabla congelada y su
  búsqueda.

  Lo que de verdad las mantiene iguales es el **DELETE de lo que ya no está declarado**. Sin
  él, el código deja de ofrecer un valor que la base de datos sigue aceptando, y nadie ve la
  diferencia. El demo lo mide: quita una moneda del manifiesto, vuelve a aplicar y comprueba
  que desapareció de la tabla.

  Las entradas van en el manifiesto a propósito: un catálogo cuyos valores solo existen en la
  base de datos es un catálogo que nadie puede revisar —añadir uno es un `INSERT` que alguien
  corrió, no un diff que alguien leyó— y el código tampoco puede conocerlos.

  `verify` rechaza una entrada con hueco, un campo no declarado, una llave repetida, un tipo
  que no encaja y un catálogo sin base de datos detrás.

### Corregido

- `docker compose up --wait` cuenta como muerto un contenedor que termina y al que nadie
  espera, así que el job del catálogo tumbaba el arranque. Ahora la app y el pooler esperan a
  **todos** los trabajos que tocan el esquema y no solo al último — que además es lo cierto:
  el servicio lee una lista que su código da por sembrada.

### Cambiado

- El demo pasó a **28 secciones y 72 comprobaciones**; la suite, a **97 pruebas**.

## [0.17.0] — 2026-09-09

### Añadido

- **`[cache]`: declarar una caché, y sobre todo lo que la vuelve mentira.** Una caché no es
  otro motor de almacenamiento: es una **copia derivada**, y lo único difícil es saber
  cuándo dejó de ser cierta. Eso es justo lo que axon sabe y una librería no, porque los
  eventos están declarados.

  ```toml
  [cache.item]
  of = "getItem"
  key = ["tenantId", "itemId"]
  ttl_ms = 2000
  invalidated_by = ["item.changed@v1"]
  enabled_by = "cache_items"
  ```

  Cada regla existe por un fallo **sin síntoma** —la respuesta equivocada, servida rápido,
  con todos los tableros en verde—:

  - el **inquilino en la llave** si el servicio es multi-inquilino: sin él, el primero que
    pregunta calienta la entrada y al siguiente se le sirve la ajena *como acierto*, y ni el
    RLS ni el router ven esa segunda consulta;
  - la llave se tiene que poder **construir desde el evento**, o el `del` no borra nada y lo
    viejo se sirve hasta el TTL. Esta la encontró el propio generador cayendo en ella:
    escribía una llave con `undefined` dentro;
  - el servicio tiene que **poder oír** lo que la invalida, o es una invalidación que nadie
    corre y que se lee como resuelta;
  - `ttl_ms + stale_ms` cabe en el `max_staleness_ms` declarado;
  - `strong` y una caché es una contradicción;
  - `pii` en la respuesta necesita cota;
  - `strategy = "refresh"` reescribe la entrada desde el evento, así que el evento tiene que
    traer la respuesta entera: un hueco se sirve igual que un dato;
  - y **la compensación también invalida**. Ese es el caso de las transacciones
    distribuidas, y es derivable porque `compensates` está declarado.

  Sale generada la llave, el envoltorio con su TTL y su single-flight, y `invalidateOn`
  enganchado al `dispatch`. Se levanta en los cuatro objetivos —valkey en local y k8s,
  Memorystore en gcp, ElastiCache en aws—, sin volumen, sin réplica y sin snapshot: nada
  aquí sobrevive a perderlo, y una caché restaurada es una caché llena de respuestas que
  eran ciertas ayer.

  Medida contra contenedores: el demo lee la llave del Valkey, comprueba que el segundo
  inquilino pidiendo **el mismo** ítem recibe otra entrada, que el PTTL es el del manifiesto
  y que con la bandera en off no se guarda nada. **27 secciones, 70 comprobaciones**, y la
  suite en **96 pruebas**.

## [0.16.0] — 2026-09-09

### Añadido

- **El sharder también en `gcp` y `aws`, como sidecar.** Era la última celda de la tabla de
  targets que decía «se niega». Ahora salen N instancias gestionadas —una **instancia** por
  nodo, porque un shard que comparte motor con los otros tres comparte su techo, su CPU y su
  caída— y pgdog al lado de la app.

  Al lado y no como servicio propio: Cloud Run y ECS sirven HTTP, y el protocolo de Postgres
  necesita un proceso que la app alcance en `localhost`. En Cloud Run son dos secretos y dos
  volúmenes, porque un volumen toma sus archivos de UN secreto y pgdog lee dos. En ECS es la
  misma forma que en k8s: un contenedor que escribe la configuración y termina, y el sharder
  esperándolo con `dependsOn`.

- **La negativa general se convirtió en una con números.** Esa forma cambia la aritmética:
  con un sidecar el pool deja de ser uno y pasa a ser uno **por instancia**, y esa
  multiplicación es lo que tumba una base de datos el día que escala.

  ```console
  $ axon infra manifests/ --target gcp
  axon: orders: on `gcp` the sharder is a sidecar …, so the pool is one PER INSTANCE:
  40 x 10 instances = 400, plus 2 reserved, over the limit of 100 per node. Lower
  `[pooler] pool_size`, lower `max_instances`, or raise the node's `max_connections`
  ```

  Nombra el número, de dónde sale y las tres salidas. La anterior solo decía que no.

  El `DATABASE_URL` sale de un secreto **distinto** (`<servicio>-pooler-url`) para que nadie
  apunte el servicio a un nodo reusando la URL vieja: eso se salta el sharding y funciona,
  contra un cuarto de los datos.

  Las dos formas pasan por `terraform validate` con los providers reales en la suite: el
  sidecar es un segundo contenedor, un volumen desde un secreto y un `dependsOn`, y un
  atributo inexistente ahí solo se vería en el `apply`.

### Corregido

- **La comprobación del ingest de Vector era intermitente**, que es peor que no tenerla:
  enseña a volver a lanzar el CI. NATS core es fire-and-forget, así que ahora se publica
  hasta que el **contador del broker** dice que entregó —y solo se repite cuando no entregó
  nada, así una segunda copia seguiría saliendo como una segunda fila y fallando—. Y la
  espera por la fila es la que el propio archivo generado pide: cinco sinks con 256 MB de
  buffer en disco tardan en arrancar en un runner cargado.

  Dos cosas que aparecieron al escribirlo: `subsz` contesta con el JSON indentado, así que
  el contador leía siempre 0 y el diagnóstico decía lo contrario de lo que pasaba; y
  `[ cond ] && break` bajo `set -e` aborta el script sin mensaje cuando la condición es
  falsa — estaba en cuatro bucles.

## [0.15.0] — 2026-09-09

### Añadido

- **El generador de Go es nativo: `axon build --lang go`.** Era un plugin, y serlo fue el
  punto durante un tiempo —demostraba que el protocolo aguanta un generador de verdad y no
  solo un check de tres líneas—. Lo que no podía demostrar es la afirmación que sostiene el
  proyecto entero, que **el manifiesto no es TypeScript disfrazado**, porque nadie corre un
  generador que primero tiene que compilar.

  No es el plugin portado tal cual: gana las tres cosas que el generador de TS ya tenía y
  que no eran comodidad sino garantías.

  - **`uses` estrecha de verdad.** El tipo del evento consumido lleva SOLO los campos que
    ese servicio declaró que lee; los demás no existen de ese lado. Y `uses = []` sale como
    un struct vacío, que es exactamente lo que hace una compensación que no mira la
    respuesta.
  - **Los fallos declarados**, con el `retriable` saliendo del manifiesto. En TypeScript un
    código no declarado no compila porque el tipo es una unión de literales; Go no tiene ese
    tipo, así que el contrato es un tipo `string` con nombre y una constante por fallo. Una
    conversión se lo puede saltar, y el comentario generado lo dice en vez de fingir: lo que
    no puede desviarse es el status y el `retriable`.
  - **Los scopes**, con el `insufficient_scope` de la RFC 6750 y no un 403 pelado.

  Idiomático y no traducido: una interfaz que la persona implementa en vez de herencia,
  `ctx` primero y `error` último, `OrderID` y no `OrderId`. Y la salida sale **ya
  formateada**, no formateable: la suite corre `gofmt -l` y `go vet` de verdad sobre dos
  servicios. Alinear como alinea gofmt no es cosmética — es la diferencia entre que el
  arnés sirva y que no.

### Eliminado

- `plugins/axon-gen-go`. Dos implementaciones del mismo generador se separan. El protocolo
  no cambió y la puerta sigue igual: `axon-gen-<lang>` para cualquier otro lenguaje, con el
  mismo `{manifest, peers}` por stdin. `plugins.md` cuenta a dónde se fue y por qué.

## [0.14.0] — 2026-09-09

### Añadido

- **`axon analytics --metabase --check`: el drift del tablero, al revés.** axon emitía las
  preguntas y las comparaba contra el manifiesto; la que escribe alguien **a mano** en
  Metabase, contra una tabla que axon posee, era invisible. El día que la columna cambia esa
  pregunta se rompe y nadie se entera hasta que la abre.

  Ahora entra lo que devolvió el tablero —su propio `/api/card`, o la forma que axon emite—
  y se cruza contra el esquema que genera, leído **del DDL mismo** y no de una segunda lista
  que se separaría al primer cambio. Sin credenciales, como todo lo demás: quien tiene el
  Metabase exporta, el compilador diffea.

  El fallo más común no es un dedazo: es la columna **PII**. El manifiesto dice que ese
  campo viaja hasheado, así que no existe con ese nombre, y la pregunta por el correo en
  claro contesta con error para siempre. Por eso el mensaje dice cuál **sí** está: eso
  convierte el error en el arreglo.

  Y lo que se niega a contestar queda contado y nombrado como **no comprobado**: una
  pregunta que no es nativa nombra tabla y campos por id numérico, y una que hace `JOIN` o
  lee un CTE vuelve ambiguo un nombre suelto. Van en una línea por grupo y no una por
  pregunta, porque un Metabase trae docenas de preguntas de ejemplo propias y cuarenta
  avisos sobre la base de muestra de otro es como una regla deja de leerse.

### Cambiado

- El demo escribe una de esas preguntas contra el Metabase que ya aprovisiona y comprueba
  que axon la nombra: **66 comprobaciones**. La suite pasó a **94 pruebas**.

## [0.13.1] — 2026-09-09

### Corregido

- **El demo esperaba con un `sleep` a que Vector se suscribiera.** El CI se cayó con
  `0 rows for one event`: la línea «Vector has started» sale **antes** de que la suscripción
  quede registrada, y un evento publicado al vacío se ve exactamente igual que una tubería
  que no funciona. En mi máquina el margen alcanzaba; en el runner no.

  Ahora la espera es contra `/subsz` del propio broker, que dice quién está suscrito: deja
  de ser una carrera y comprueba de paso algo que antes se daba por hecho —que las dos
  réplicas entraron al **mismo** grupo de cola—. Y cuando falla ya se ve por qué: la salida
  del publicador y los logs de las dos réplicas.

  El demo pasó a **65 comprobaciones**.

## [0.13.0] — 2026-09-09

### Añadido

- **El ingest de Vector, medido contra contenedores.** Era el último generador que se
  quedaba en «valida»: `vector validate` dice que el archivo está bien formado, no que lleve
  un evento del broker a la bodega, y menos que hashee lo que tiene que hashear. El archivo
  generado ya nombra los contenedores que levanta el target local, así que el demo lo corre
  **tal cual**, sin reescribir nada, y la imagen sale del generador y no del script: dos
  lugares nombrando una versión se separan, y lo que se mediría entonces es otro Vector que
  el que despliega el manifiesto de k8s.

  **Dos réplicas a propósito.** El grupo de cola era un comentario en un archivo generado
  hasta que algo lo midió: quitando `queue` y repitiendo salen **2 filas para un evento**,
  que es el embudo contando cada flujo dos veces sin que nada se vea mal.

  **Y el hash contra el del otro camino.** La misma tabla la llena el cargador SQL con su
  propia expresión (`lower(hex(SHA256(salt || email)))`) mientras Vector hashea con VRL
  (`sha2(…, variant: "SHA-256")`). Dos ingestas con hashes distintos para la misma persona
  son dos columnas que nadie puede juntar, y ninguna de las dos se ve mal por separado.

  Su propia fila se borra antes y después: ese evento no tiene cobro detrás, y dejarla haría
  que el embudo de la **siguiente** corrida contara un flujo que no convirtió. Comprobado
  corriendo el demo dos veces seguidas.

### Cambiado

- El demo pasó a **26 secciones y 64 comprobaciones** contra contenedores reales.

## [0.12.0] — 2026-09-08

### Añadido

- **`axon pact` lee también los pactos de mensaje.** Hasta ahora solo leía `interactions[]`,
  o sea pactos HTTP, y un endpoint es la mitad de la superficie. Un **topic** también es un
  contrato, y quién lee un mensaje —y qué campos— no se observa desde este lado ni siquiera
  como el edge observa una llamada: o lo dicen, o no se sabe. Entran los `messages[]` de v3
  y las interacciones con `type: Asynchronous/Messages` de v4, y se cruzan contra lo que el
  proveedor emite: la misma comparación que hace `uses` para un consumidor de dentro, con la
  entrada viniendo de fuera.

  El topic se lee de `topic`, `kafka_topic`, `subject`, `destination` o `queue` —no hay una
  sola clave, cada implementación escribe la suya— y empareja las dos grafías: el pacto dice
  `order.placed.v1` y el manifiesto `order.placed@v1`.

  Un evento que existe pero es de **otro** proveedor se reporta como eso y no como
  inexistente: un evento tiene exactamente un dueño y el arreglo es distinto. Y si el mensaje
  lleva el sobre, lo que el evento promete es lo de dentro de `data`; leer del sobre algo que
  no viaja ahí es un hallazgo aparte.

### Cambiado

- El resumen de `axon pact` cuenta las dos mitades (`N interactions, M messages`), y el demo
  pasó a **25 secciones**: comprueba el pacto de un consumidor HTTP y el de uno de un topic.
- La suite pasó a **93 pruebas**.

## [0.11.0] — 2026-09-08

### Añadido

- **El sharder se levanta también en k8s.** `--target k8s` se negaba si el manifiesto
  declaraba `shards > 1`, así que un servicio sharded solo existía en el portátil. Los
  nodos **no** son de axon —son instancias gestionadas del equipo y llegan como secreto—
  pero pgdog sí: es la pieza que hay que configurar exacta, y hacerlo a mano es donde un
  shard acaba apuntando al host equivocado. Salen un `ConfigMap` con los dos archivos
  generados, un `Deployment` de pgdog pineado por digest, su `Service` en 6432 y una
  `NetworkPolicy` donde solo entra su propio servicio.

  El `DATABASE_URL` de la app apunta al pooler y **nunca** a un nodo: apuntar a un nodo se
  salta el sharding y todo funciona, contra un cuarto de los datos.

- **La sustitución se genera con los marcadores.** El archivo generado es una plantilla
  (`port = ${AXON_DB_PORT_0}` ni siquiera es TOML válido) y hasta ahora lo único que
  rellenaba esos marcadores era la regex del propio test: en un despliegue real no lo
  hacía nadie. Ahora la sustitución sale del mismo texto que los marcadores, viaja como
  initContainer, y una variable sin valor **para el pod diciendo cuál falta** en vez de
  dejar un `${…}` literal que pgdog rechaza con un error de parseo que nadie relaciona con
  un secreto.

  Medido: la suite corre esa sustitución con un `sh` de verdad —con una contraseña que
  lleva `/`, que es justo lo que rompe un `sed` con delimitador `/`— y valida lo que queda
  contra el esquema oficial de pgdog.

- **El plan neutral lleva la configuración del pooler** (`stores[].pooler`, con sus dos
  archivos y la lista de variables), así que un `axon-infra-*` que renderice a Nomad o a
  lo que sea no tiene que volver a derivarla. Está en el esquema publicado.

### Cambiado

- `axon pooler --target k8s` es un target válido: la cabecera del archivo generado nombra
  ese comando, y un comando que no se puede ejecutar no es documentación.
- `gcp` y `aws` siguen negándose con `shards > 1`: allí son N instancias de Cloud SQL o
  RDS. La suite pasó a **92 pruebas**.

## [0.10.0] — 2026-09-08

### Añadido

- **`axon ci --forge gitlab`.** El pipeline generado solo sabía de GitHub Actions, así que
  un equipo en GitLab tenía que reescribir a mano las compuertas —que son de axon, no de
  la forja— y lo que se reescribe a mano se pierde. Ahora el mismo pipeline sale en el
  otro dialecto: las mismas tres compuertas (`verify` contra **todos** los manifiestos,
  código generado al día, migraciones en dry-run), deploy solo desde la rama por defecto,
  OIDC vía `id_tokens` en vez de una llave larga guardada en una variable, y deploy por
  digest.

  `[ci] image` es el único campo que no es portable, y por eso **falla en vez de emitir**:
  el `${{ }}` de GitHub es texto literal para GitLab, así que el deploy subiría una imagen
  cuyo tag es la expresión misma y nadie se enteraría hasta que alguien leyera el
  registro. El campo pasa a ser opcional —cada forja tiene su default— y generar GitLab
  con sintaxis de GitHub aborta diciendo por qué.

  Comprobado con un parser de YAML de verdad, no con búsqueda de subcadenas: que las tres
  compuertas siguen ahí, que el deploy no se dispara desde un merge request y que el
  `--target` no filtra otra nube.

### Cambiado

- La suite pasó a **91 pruebas**.

## [0.9.0] — 2026-09-08

### Añadido

- **`mode = "apply"`: el lazo cerrado, con dos cerrojos.** La regla mueve la palanca ella
  misma, y hace falta que el manifiesto diga que **puede** y que quien la corre diga que
  **ahora** (`axon rules --apply`): ninguno solo hace nada, porque contestan preguntas
  distintas y un solo interruptor las confundiría. `verify` no lo bloquea y tampoco se
  calla — lo nombra como lazo de control sobre producción cada vez que alguien lee la
  salida.

  Solo mueve un **flag**, sobre la configuración de flagd que axon mismo generó: cualquier
  otro almacén es la API y la credencial de otro. La palanca **vuelve sola** cuando la
  condición se levanta, o se queda donde la dejó el peor día del trimestre. Y cada cambio,
  en los dos sentidos, es una línea en la auditoría con lo que la regla leyó y por qué: un
  cambio automático en producción que no deja rastro es la peor versión de esto.

  Medido contra flagd y no contra el archivo: el demo tira la métrica, aplica, le pregunta
  a flagd qué sirve, la recupera y comprueba que la palanca volvió.

### Cambiado

- El demo pasó a **24 secciones y 62 comprobaciones**.

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
