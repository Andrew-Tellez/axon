# Changelog

Formato de [Keep a Changelog](https://keepachangelog.com/es/1.1.0/);
versionado según [SemVer](https://semver.org/lang/es/).

El **formato del manifiesto** todavía puede cambiar de forma incompatible antes de
`1.0.0`. La superficie de comandos es estable: un comando puede ganar banderas, no
perderlas.

## [0.41.0] — 2026-09-20

### Añadido

- **El baseline guarda la topología de shards.** Cambiar `shards = 4` a `shards = 16` pasaba
  en silencio, con `axon.baseline.json` generado y todo, y es probablemente el edit más
  peligroso de un manifiesto: el sharder coloca una fila hasheando su `shard_key` módulo ese
  número, así que toda fila ya escrita pasa a hashear a otro nodo. La query va al nodo nuevo,
  no encuentra nada, y devuelve **vacío en vez de un error**.

  `verify` obliga a declarar `shard_key`, obliga a `tenant_binding = "set_local"` y obliga a
  admitir que con varios nodos la consistencia real es eventual —y después dejaba cambiar el
  divisor del hash sin decir una palabra.

  Subir y bajar no comparten mensaje porque no son el mismo desastre: al subir, el dato sigue
  donde estaba y nadie lo encuentra; al bajar, lo que vive en los nodos que desaparecen deja
  de ser alcanzable. Los dos contestan vacío sin error, que es exactamente lo que los hace
  difíciles de ver. Un servicio sharded que todavía no está en el baseline es un warning, no
  un error, igual que cualquier otro contrato sin registrar.

  Va en el mismo archivo que los contratos aunque no sea un contrato con un llamador. Es un
  contrato con los datos.

## [0.40.1] — 2026-09-20

### Corregido

Los cinco salieron de pasar la CLI recién liberada por el banco de pruebas, que es
exactamente para lo que existe. Cuatro son de lo que 0.40.0 acababa de traer.

- **`axon docs` no dibujaba la máquina de estados.** Para quien integra, el orden de las
  operaciones **es** el contrato: llamar `pay` sobre una factura cancelada falla, y el
  manifiesto lo declaraba desde siempre sin que el documento lo dijera. Ahora sale como una
  tabla de `call | from | to`, con solo las transiciones que un llamador puede causar —una
  que dispara un evento que él no emite es asunto del servicio, no suyo.

- **La guía de un servicio sin métodos se contradecía a sí misma.** Decía «checks only the
  scopes below» sin scopes abajo, «it reacts to events —see below—» sin consumir ninguno, y
  ofrecía OpenAPI «for the routes above» sin rutas. Y no decía lo único que un integrador
  necesitaba saber: que era un job con horario. Ahora distingue los tres casos —el cron, el
  consumidor de eventos y el que solo llama— la sección de autenticación desaparece cuando
  no hay nada alcanzable, y el pie no ofrece un OpenAPI que estaría vacío.

- **El job `cumplimiento` de `axon ci` reportaba de más.** Corría `axon compliance` sobre el
  repo entero dentro de un pipeline que es de un servicio, así que el artefacto de `billing`
  hablaba de los seis. `axon compliance` gana `--service`, como `docs` y `catalog` ya tenían,
  y el pipeline lo usa.

- **`axon docs --service <externo>` decía que no era un servicio.** Sí lo es; es externo, y
  eso es otra cosa. Un manifiesto `external` es la copia congelada de la API de alguien más,
  y su guía de integración es de quien la escribió. Ahora lo dice, y apunta a `axon discover`
  para lo que sí es de esta plataforma: qué le llama.

### Añadido

- **Un `GET` que mueve el estado es un warning.** Salió de que el banco declaraba la
  transición `ship` colgando de `getOrderV2`: data de relleno que llevaba meses ahí y que
  nadie había visto, porque hasta ahora nada dibujaba el ciclo de vida. Un read que cambia el
  estado se cachea, se prefetchea y lo reintenta cualquier cliente con timeout — la
  transición vuelve a ocurrir y nadie la pidió dos veces.

- **La guía de integración, servida en la página.** Es la salida real de `axon docs` sobre
  `examples/`, regenerada antes de cada `mdbook build`, por la misma razón y en el mismo
  lugar que el playground compila el wasm: un ejemplo pegado a mano se queda viejo la primera
  vez que el generador cambia una palabra, y nada falla.

## [0.40.0] — 2026-09-18

### Añadido

- **`axon docs <fuentes>`: la guía de integración, para quien va a llamar un servicio que
  no escribió.** `axon openapi` ya describe las rutas y sus formas, y de ahí sale un
  cliente. Lo que no carga es todo lo que un integrador pregunta antes del primer request:
  qué emisor firma el token, cómo se llama el claim del tenant, a qué eventos puede
  suscribirse en vez de encuestar, qué pasa cuando el otro lado está particionado. Todo eso
  estaba declarado —en `[auth]`, en `[emits]`, en `[cap]`— y solo existía como código
  generado que nadie fuera del repo lee.

  Markdown y no otro JSON a propósito: el lector es una persona haciendo onboarding o un
  agente leyendo contexto, y los dos van mejor con un documento que con cuatro comandos
  cuyas salidas tienen que unir. También es la quinta herramienta del servidor MCP.

  **No hay bloque `[docs]` y no debería haberlo.** Prosa que no se deriva de una
  declaración es prosa que se pudre sin que nada falle, que es exactamente el problema que
  el manifiesto existe para resolver. Lo que no está declarado, el documento lo dice —«este
  servicio no declara `[auth]`, pregúntale a quien opera el gateway»— en vez de inventarlo.

- **`axon compliance <fuentes>`: la matriz de controles.** Un auditor no pide el código.
  Pide qué control cubre un requisito, dónde se aplica y cómo sabes que no se desvió. El
  manifiesto ya contesta la mitad —quién puede llamar qué, qué campos son personales,
  cuánto se guarda un respaldo, qué campos puede recibir un consumidor— y la respuesta
  estaba ahí sin leerse.

  Cinco regímenes: HIPAA, SOC 2, ISO/IEC 27001, la LFPDPPP y PCI DSS. Un control es **una
  propiedad del sistema**; lo que cambia entre regímenes es cómo le llaman, así que cada
  uno carga su propia cláusula y `--framework soc2` imprime `CC6.1`, no la cita de la ley
  de al lado.

  **No es una certificación, y el reporte lo dice en el encabezado.** El cifrado en reposo,
  el TLS, dónde caen los logs de acceso y si la gente tomó el entrenamiento vuelven
  `manual`, con la pregunta que una persona tiene que contestar. Marcarlos en verde sería
  peor que no tener la herramienta: sería una auditoría que pasa sin que nadie mire.

  El que más se gana con estar declarado es *minimum necessary* (HIPAA §164.502(b)): `uses`
  ya lo implementaba sin llamarlo así. Declaras tres campos, el tipo generado trae tres, y
  un campo que nadie pidió **no compila**. No es una política escrita, es el compilador.

- **Dos niveles para declarar un régimen, porque son dos preguntas distintas.** SOC 2 e ISO
  27001 son de la organización: todo servicio está en alcance y `frameworks` en
  `axon.policy.toml` lo dice una vez. PCI DSS es de un servicio —solo el que habla con el
  procesador está en alcance de la tarjeta— y va en `compliance` del manifiesto. Un control
  se evalúa contra la unión.

  Sin esa separación, declarar un régimen de dominio a nivel repo le pedía una llave de
  sellado a un catálogo de productos, y un reporte de cumplimiento con ruido es un reporte
  que nadie lee. `compliance` no se serializa vacío, por lo mismo que `workflow`: el
  manifiesto viaja embebido en cada contrato generado y una llave nueva es un diff en todos
  los archivos de todos los repos que regeneren.

- **`[framework.*]` en la policy: los regímenes del equipo.** No agregan controles —eso es
  un release— le ponen **su** cláusula a un control que ya existe. La asimetría es el
  punto: un equipo mapea sus obligaciones sobre lo que el compilador de verdad revisa, y no
  puede pintar algo de verde escribiéndolo en un archivo de configuración.

  Las llaves son ids de control, que `axon compliance --ids` lista. La lista no está escrita
  a mano: se **evalúa** contra un manifiesto de referencia que dispara todas las
  condiciones, así que no puede desviarse el día que se agregue un control. Un mapeo que
  nombra un id inexistente se rechaza en vez de ignorarse: es un control que alguien cree
  cubierto.

- **`PodDisruptionBudget` en `--target k8s`, donde hay réplicas que perder.** Solo con
  `min_instances > 1`. Sobre una sola copia, `minAvailable: 1` es un drain que nunca
  termina: el nodo espera por un pod que no tiene permiso de desalojar, y el upgrade se
  atora en un servicio que nadie notó que era copia única. Sin presupuesto es mejor que con
  uno imposible.

- **`axon ci` publica los dos reportes.** La compuerta ya existía: con los regímenes en la
  policy, `axon verify` rechaza un hueco igual que un evento que nadie consume. Lo que
  faltaba era la evidencia — un check verde no contesta qué era cierto en una fecha.

  `cumplimiento` solo se emite si hay regímenes declarados, y va en el `needs` del deploy.
  `documentacion` sale siempre y **no** va: una guía que no renderizó no es razón para
  detener un release, y cablearla como compuerta es cómo un equipo aprende a saltarse las
  compuertas. Los dos escriben al summary del run y no solo a un zip: evidencia que nadie
  abre es evidencia que nadie revisó.

  El artifact es comodidad. El registro durable es el commit: la matriz se deriva de los
  manifiestos, así que se regenera desde cualquier sha años después sin depender de un
  ajuste de retención que alguien cambió.

### Cambiado

- El pipeline de GitLab gana una etapa, `docs`, entre `test` y el deploy. Las compuertas
  siguen siendo las mismas y siguen en `contracts`.

## [0.39.0] — 2026-09-17

### Añadido

- **`axon workflows <manifiesto> <fuentes>`: los flujos de Temporal, como su propio
  módulo.** Salían dentro de `axon build`, y eso estaba mal por una razón que es de
  Temporal y no una preferencia: el worker carga el código del workflow en un bundle
  aislado, así que no puede vivir en el mismo archivo que el transporte, los clientes y los
  tipos de la base de datos. Lo que necesita del contrato entra como `import type`, que se
  borra al compilar. El contrato dice adónde se fueron y no los importa.

### Corregido

Los tres los encontró el banco de pruebas al declarar en él los cinco bloques nuevos, que
es exactamente para lo que existe.

- **`axon seq <flujo>` no sabía dibujar un `[workflow]` de Temporal.** Contestaba «nadie lo
  emite y ningún saga se llama así», con la lista de sagas que venía justo después
  **vacía**: el flujo existía y el mensaje decía lo contrario. Ahora se dibuja, y lo que se
  dibuja es la mitad que un saga no tiene — el timer como una nota (nadie está esperando
  ahí dentro) y la señal como una flecha que **entra**, desde quien emite el evento. En una
  revisión, un flujo que espera dos días y se rinde es una decisión; repartido en tres
  claves de un manifiesto es un número que nadie lee.

- **Un `[sse]` no salía en el OpenAPI.** Un socket no se puede expresar ahí y está bien que
  falte; un stream es un `GET` corriente, así que quien lee el documento no veía el endpoint
  que tiene que abrir. Sale con `text/event-stream`, sus parámetros de ruta y la descripción
  de cómo viaja cada trama.

- **Un `[workflow]` de Temporal dejaba el contrato entero sin cargar.** El generador
  importaba `sleep` de `@temporalio/workflow`, y `sleep` ya era el nombre del backoff de
  los reintentos en ese mismo archivo: dos declaraciones de un nombre en un módulo no
  parsean. El síntoma era que el testkit —que prueba cosas que no tienen nada que ver con
  Temporal— no arrancaba. `tsc` no lo vio porque el módulo que no podía resolver tapaba
  todo lo demás, y ninguna prueba leía el archivo generado como archivo. Arreglado por los
  dos lados: el import va como `timer`, y los workflows se fueron a su propio módulo. Hay
  un gate nuevo que lee cada archivo generado y se niega si un nombre está declarado dos
  veces.

- **El `test_cmd` que escribía `axon init` no corría en el proyecto que `axon init`
  escribe.** La policy decía `node --test services/{service}`, y node no busca dentro de un
  directorio: intenta **ejecutarlo**, y falla con `MODULE_NOT_FOUND` — que no es «no hay
  pruebas», es un error. Encima no había ningún archivo de prueba ahí dentro, porque el
  testkit generado es una librería que se cablea desde uno propio y ese archivo no existía.
  El pipeline que emite `axon ci` salía rojo en el primer build de cada proyecto nuevo.

  Arreglado por los dos lados: el default es un glob, y `init` escribe
  `services/{service}/axon.test.ts` con una aserción de verdad —que el contrato sirve las
  rutas que el manifiesto declara— y el comentario que dice cómo cablear el testkit ahí
  mismo. No lo vio nadie porque cada prueba de la suite comprueba lo que axon **imprime**;
  la nueva comprueba lo que eso impreso **hace**.

- **`httpRoutes` desaparecía en vez de venir vacío.** Un servicio que no sirve ninguna ruta
  —un consumidor, un job— generaba un módulo **sin** ese export, así que todo lo que lo
  importaba dejaba de cargar con un `SyntaxError` sobre un nombre que falta en vez de leer
  una lista vacía. El andamio que escribe `axon init` lo importa, de modo que quitar la
  última ruta de un manifiesto era un proyecto que dejaba de arrancar. La superficie de un
  módulo generado no puede depender de los datos: quien lo importa escribió su `import` una
  vez.

- **`axon tui` no mostraba ninguno de los cinco.** El panel listaba base de datos, caché,
  índice, eventos y rutas, y ni el bus, ni el socket, ni el stream, ni las tareas, ni los
  flujos. Un socket y un stream son puertas también, y una topología que dibuja solo las de
  HTTP enseña un servicio con menos formas de entrar de las que tiene.

## [0.38.0] — 2026-09-17

### Añadido

- **`[sse.<name>]`: la otra dirección.** Lo que un transporte de petición/respuesta no
  tiene, y lo que lo hace declarable —donde un push de WebSocket no lo es— es que lo que
  viaja por ahí **no es nuevo**: son los eventos de `[emits]` y `[consumes]`, con sus tipos
  y sus campos ya escritos. La trama la tipa el dueño del evento, y un stream no reescribe
  el evento de otro.

  Se genera el formato de cable, que es quisquilloso y vale la pena acertarlo una vez: el
  `id:` que vuelve como `Last-Event-ID`, el `event:`, la única línea `data:` y la línea de
  comentario que es un heartbeat. No se genera si ese evento le toca a **esa** conexión —el
  inquilino de la conexión contra el del evento—: es la única parte que sabe del dominio,
  así que es un método abstracto y no un default, porque los dos defaults disponibles son
  todo para todos y el silencio.

  Y la regla que paga el bloque entero: un stream `public` que empuja un evento con un
  campo `pii` es dato personal publicado, no expuesto.

- **`[ws]`: el mismo método, sobre un socket.** `ws = "<tipo>"` en un método es un segundo
  transporte y no un segundo método: el mismo `in`, el mismo `out` y **la misma
  implementación**, que es justo el punto — un cuerpo por transporte es lo que dejaría que
  HTTP y el socket contestaran distinto. Sale un `dispatchWs` que parsea la trama, la
  enruta por tipo y contesta correlacionado por el id que mandó el cliente; sin ese id, un
  cliente con dos peticiones en vuelo no puede saber cuál respuesta es de cuál, y un socket
  no tiene el emparejamiento de HTTP al que recurrir.

  Los mandos viven en el bloque y no en el edge porque el edge no puede aplicarlos: después
  del upgrade ha visto **una** petición, y todo lo que viaja por esa conexión le es
  invisible. Por eso `rate_limit` es por conexión, por eso hay un techo de trama, y por eso
  `origins` existe — CORS no aplica a un WebSocket: el handshake no es una petición que el
  navegador bloquee, así que cualquier página puede abrirlo y `Origin` es lo único que dice
  de dónde vino.

  Lo que no es: la otra dirección. Un servidor que empuja a una conexión que no pidió nada
  es fan-out, y el fan-out necesita saber qué conexión está en qué proceso — un registro que
  axon no modela.

### Cambiado

- **La página de patrones cubre los flujos durables y la cola de tareas.** Los dos bloques
  salieron en 0.37.0 y la página no los nombraba, así que la fila de la tabla remitía a un
  sitio donde no estaban. Lo que se cuenta no es la lista de llaves —esa está en la
  referencia del manifiesto— sino el porqué: qué es lo que un saga no puede hacer y por
  qué esas dos cosas solo existen contra un historial, que `engine = "saga"` no es una
  segunda implementación sino el coordinador de siempre, que `engine = "temporal"` es un
  worker y no un runtime que axon despliegue, y la regla de la forma contra la baseline
  con lo que entra en la huella y lo que no. De la cola, por qué es una tabla del propio
  servicio —la misma razón que el outbox— y el `claim` entero explicado, que es lo único
  ahí que no se puede escribir a la ligera.

- **La referencia del manifiesto cubre `[bus]`, `[workflow]`, `[tasks]` y las cuatro
  llaves nuevas de `[consumes]`.** Tres secciones con la forma que tiene el resto de esa
  página: el bloque declarado, la frase que dice para qué es, y la tabla de lo que
  `verify` refuta. La del bus es la que más aporta, porque es la única que dice en voz
  alta que en una nube gestionada el bus es el de la nube y un `kafka` declarado se
  rechaza en vez de traducirse en silencio.

- **La página de infraestructura dice qué levanta cada objetivo con el bus declarado.** La
  tabla de arriba seguía diciendo «NATS JetStream» para `local`, que ahora es el motor que
  se declare; y la lista de crons no mencionaba el drenado de la cola de tareas. Con ello,
  por qué los comandos que crean los topics y los consumidores salen como comentarios y no
  como un contenedor que los ejecute, y qué levanta —y qué no— un flujo `temporal` en cada
  objetivo.

## [0.37.1] — 2026-09-16

### Corregido

- **El presupuesto de un `[workflow]` no contaba la espera de una señal.** Sumaba las
  llamadas, sus compensaciones y los timers, pero no el `timeout_ms` de un `awaits`, así
  que un flujo que puede esperar dos días a que llegue el evento pasaba con un presupuesto
  de quince minutos. Lo que habría hecho en producción es rendirse en mitad de la espera y
  compensar algo que no había fallado — que es exactamente lo que esta regla existe para
  impedir del otro lado.

  Lo encontró `axon verify` sobre el ejemplo que se estaba escribiendo para el README: la
  regla vio el error antes que quien la escribió.

### Cambiado

- **El README cuenta los cuatro bloques de 0.37.0.** Una sección nueva con la razón de cada
  uno y no su lista de llaves: qué se rompe si dos servicios no coinciden en el bus, por
  qué la cola de tareas es una tabla del propio servicio, y qué refuta `verify` sobre la
  forma de un flujo que tiene instancias en vuelo.

## [0.37.0] — 2026-09-15

### Añadido

- **`[workflow]`: un saga que además puede esperar.** Con `engine = "saga"` —el que no se
  declara— no hay nada nuevo: se baja al coordinador sobre Postgres que ya existía, y
  hereda su tabla, su sweep y sus quince reglas. Lo que trae el bloque son las dos cosas
  que ese coordinador no puede hacer, porque solo existen contra un historial: un timer
  durable (`sleep_ms`, que no es un proceso esperando un día, es una fila con una fecha) y
  una señal (`awaits`, que es un evento que el servicio ya consume). Con
  `engine = "temporal"` sale el worker —una política de reintentos por paso, el `sleep`, el
  `condition` sobre la señal y las compensaciones en orden inverso— y el servidor con su UI
  en el compose local. En k8s solo la dirección: un Temporal en un manifiesto generado es
  un archivo que nadie puede operar.

  La regla que justifica todo lo demás es `version` contra la baseline. Un worker que
  reproduce un historial viejo contra código nuevo no falla donde está el cambio: falla
  donde el replay deja de coincidir, y los flujos ya empezados se quedan atascados sin nada
  en los logs que nombre la causa. Ahora eso es un error antes del deploy, y dice qué paso
  dejó de coincidir. La duración de un timer entra en la huella, porque un replay
  reprograma el mismo timer.

- **`[bus]`: el broker, declarado.** La tercera elección de infraestructura que vive en el
  manifiesto, al lado de `[cache]` y `[search]`: `nats` —el de siempre—, `kafka`, `rabbit`
  o `none`. Lo que impide que cinco servicios acaben en cinco brokers no es un archivo
  compartido que alguien tiene que acordarse de leer: es una regla que los lee todos y se
  niega nombrando a los dos que no coinciden. Un evento publicado en un bus y consumido del
  otro no falla —el emisor tiene éxito, el consumidor se calla— y la única señal es un
  handler que nunca corre.

  En `local` sale el contenedor que toca y el servicio se sigue llamando `broker`: lo único
  que cambia es `AXON_BROKER_URL`. En `gcp` y `aws` un kafka declarado **se rechaza** en vez
  de traducirse a Pub/Sub o a SQS, que es la misma decisión que ya se tomó con Meilisearch:
  cambiar el motor por detrás del manifiesto es otro orden, otra reentrega y otra falla.

- **Las colas, en `[consumes]`.** `group` —los consumidores que se reparten el trabajo—,
  `ordered_by`, `max_deliver` y `ack_wait_ms`. Son neutrales al motor y cada uno se traduce
  al suyo. Salen tres veces: en el comando que crea el consumidor, en el contrato de
  TypeScript y en el de Go, porque si no salieran, quien cablea el consumidor los teclearía
  de memoria — y el que se desincroniza, la ventana de ack, se ve como un handler que corrió
  dos veces.

  Con ellos vienen sus refutaciones: ordenar por un campo que el evento no lleva (y dice
  cuáles sí lleva), ordenar sobre un motor que no particiona, dos servicios compartiendo un
  `group` —un grupo se reparte, no se duplica: el handler del otro nunca ve la mitad—, y un
  `ack_wait_ms` por debajo del presupuesto del flujo que ese evento arranca, que es un
  segundo coordinador sobre el mismo id con los dos compensando.

- **`[tasks]`: el trabajo que no cuelga de la petición.** Casi todo lo que se llama tarea
  asíncrona es un evento que el servicio se manda a sí mismo, y para eso ya estaban
  `[emits]` y `[consumes]`. Lo que un evento no puede dar son tres cosas, y son estas:
  correr más tarde (`delay_ms`), limitar cuántas corren a la vez (`concurrency`), y
  devolver un recibo — un `taskId` que quien encoló puede consultar, cosa que un
  fire-and-forget no tiene.

  La cola es una tabla, `axon_task`, en la Postgres del propio servicio: encolar dentro de
  la transacción que cambió la fila es una escritura más y no un segundo commit que se
  puede perder. Sale el `enqueue`, el worker con su backoff y su techo de intentos, y la
  interfaz de la cola con el SQL del claim escrito en el comentario, porque es el que no se
  puede escribir a la ligera: sin `FOR UPDATE SKIP LOCKED` dos workers toman la misma fila
  y la tarea corre dos veces. `axon infra` apunta un scheduler a la ruta que la drena, que
  es lo que recupera lo que un worker muerto tenía agarrado.

### Cambiado

- **`[consumes]` rechaza una llave que no conoce.** Hasta ahora la ignoraba, que con cuatro
  llaves nuevas es justo el peor desenlace: un `max_delivers` mal escrito parece declarado,
  se lee como una decisión en una revisión, y no hace nada.

- **`axon_task` no lleva `tenant_id`,** como el `outbox` y el `inbox_seen`: es una tabla que
  escribe axon y que no es de ningún inquilino, así que las reglas de multi-inquilino la
  saltan. La lista de esas tablas estaba copiada en tres sitios y ahora es una.

## [0.36.0] — 2026-09-15

### Añadido

- **`axon completions <shell>`.** Treinta y un comandos es más de lo que nadie recuerda, y
  hasta ahora la única forma de encontrarlos era leer el `--help` entero. El script lo
  imprime el propio binario a partir de la misma definición que el `--help`, así que no
  puede quedarse atrás: completa comandos, banderas y los valores fijos que una bandera
  acepta. zsh, bash, fish, elvish y powershell. Lo que no completa es lo que solo saben los
  manifiestos —el nombre de un servicio después de `-s`.

- **Y sí completa lo que solo saben los manifiestos.** El script no lleva ninguna lista:
  pregunta al binario en cada tabulación, así que `-s ` ofrece los servicios del
  directorio y `axon seq ` los eventos emitidos, con su emisor como descripción. Lee el
  directorio actual, que es lo que asume cualquier comando sin fuentes; en uno sin
  manifiestos no hay nada que ofrecer y la tabulación se comporta como antes.

- **Go genera clientes.** Hasta ahora el Go generado eran tipos, handlers y la máquina de
  estados: quien llamaba a otro servicio escribía a mano el timeout, los reintentos y el
  cortacircuitos, que es justo lo que el manifiesto ya declara. Ahora sale el cliente, con
  la política dentro y literal — el timeout es el `context` que la llamada ya recibe, el
  backoff lleva su jitter, el cortacircuitos es un mapa detrás de un mutex, y un fallo que
  la otra parte declaró final no se reintenta. `on_partition = "degrade"` hace del camino
  degradado un argumento obligatorio, igual que en TypeScript: no se puede llamar sin decir
  qué se sirve mientras la otra parte no contesta. Y una dependencia sobre un método que se
  está muriendo sale con el `Deprecated:` que leen el editor y el linter de Go.

  Es la misma política en los dos lenguajes porque sale del mismo sitio, y hay un test que
  falla si dejan de coincidir. El servicio gana un `transport Transport` en su constructor
  cuando tiene `[[depends]]`.

- **Y las banderas.** Los accesores tipados de cada `[flags]`, con la misma forma de
  OpenFeature que en TypeScript y la única que Go tiene para ella: una interfaz no puede
  llevar un parámetro de tipo, así que el proveedor contesta `any` y una función genérica
  comprueba que lo que volvió es lo que se declaró — un proveedor que contesta una cadena a
  una bandera booleana es una mala configuración, y el valor seguro es mejor respuesta a eso
  que un panic en medio de una petición. El valor por defecto viaja **dentro del código**,
  así que un proveedor caído sigue contestando lo que el manifiesto dijo que era seguro, y
  lo que está anclado a un campo lo lleva bajo su nombre y bajo el de OpenFeature.

### Cambiado

- **Un hallazgo dice de qué servicio es y qué archivo abrir, en campos.** Hasta ahora un
  hallazgo era una frase y nada más, y quien necesitaba sus partes las sacaba partiendo
  la frase por el primer `:` — eso hacía el editor para decidir qué archivo subrayar, así
  que una regla que se redactara de otra forma caía en la línea 1 de lo que estuviera
  abierto, sin avisar. Ahora el compilador coloca cada hallazgo sobre el manifiesto del
  servicio del que habla, una vez, con la lista de manifiestos delante. El texto que
  imprime `axon verify` no cambia ni una coma; lo que cambia es `--json` y lo que devuelve
  la herramienta `verify` del MCP, donde cada entrada de `errors` y `warnings` pasa de ser
  una cadena a ser `{ message, service, file }` — `service` y `file` solo cuando el
  hallazgo nombra uno que existe en el proyecto. `axon accept` sigue anclado al mensaje.

- **Y ahora se coloca cada hallazgo, no solo los que abren con dos puntos.** Una regla que
  dice `orders calls payments.charge, which is deprecated` nombra su sujeto igual de claro
  que una que escribe `orders: ...`, pero el editor solo sabía leer la segunda, así que
  cuarenta hallazgos —todos los de la familia `[A01]`, los de dependencias y los de
  consumo— se quedaban sin archivo. Se lee la primera palabra de la frase, sin el prefijo
  `[A01]` o `[axon-check-x]`, y se comprueba contra la lista de servicios: una palabra que
  no nombra un servicio no coloca nada. Sobre `examples/` pasa de 7 de 11 a 11 de 11. Lo
  que sigue sin colocarse es lo que no es de un servicio: `[api]` es una decisión sobre
  todos los manifiestos y ponerla sobre el primero que se leyó es señalar un archivo al
  azar.

- **`stock.changed@v1 (inventory) has no consumers` pasa a `inventory: stock.changed@v1 has
  no consumers`.** Era la única regla que nombraba a su servicio entre paréntesis, y por eso
  la única cuyo aviso no se podía abrir. Si tienes esa línea en un `axon.accepted.json`,
  vuelve a correr `axon accept`.

- **Qué tipos existen se decide una vez, para todos los lenguajes.** El generador de
  TypeScript y el de Go resolvían los dos el esquema de un evento consumido contra su
  emisor, lo recortaban los dos a los campos que el consumidor declaró que lee, y los dos
  derivaban la forma de la respuesta que devuelve otro servicio. Escrito dos veces, ya
  había derivado: Go llamaba `…Result` a lo que TypeScript llama `…Out`, Go no emitía el
  tipo cuando quien llama no declaraba `uses`, y Go no emitía nunca el tipo de lo que
  *envía*. Dos nombres para un contrato es exactamente lo que esta herramienta existe para
  rechazar. Ahora esas decisiones viven en `contract.rs`, sin lenguaje dentro —el nombre
  viaja en partes, sin mayúsculas, y cada lenguaje las une como su lector espera—, y un
  generador es lo que las convierte en sintaxis. La superficie de tipos de Go pasa de 74
  líneas a 13, que es lo que costaría un tercer lenguaje.

  **Rompe el Go generado**: `PaymentsCapturePaymentResult` pasa a `PaymentsCapturePaymentOut`,
  y aparecen los `…In` y los `…Out` que faltaban. El TypeScript no cambia de tipos: solo
  gana los comentarios que Go ya tenía.

### Corregido

- **El gate de Go del suite no se ejecutaba nunca.** La comprobación de si la herramienta
  está instalada corría `go --version`, que no es una de sus dos formas: sale con código 2
  y un mensaje de uso. Así que `go build` sobre lo que escribe el generador, `gofmt -l` y
  `go vet` se saltaban solos en una máquina con Go instalado, y lo decían en una línea que
  nadie lee. Una puerta que se apaga sola es peor que no tenerla.

## [0.35.0] — 2026-09-13

### Añadido

- **Un `migrations` que no lleva a ninguna parte se rechaza.** Todas las reglas sobre el
  esquema se callan cuando no hay esquema que leer, así que una ruta equivocada apagaba de
  golpe el outbox, el inbox, las claves foráneas, los CRUD y los índices —y el manifiesto
  seguía diciendo dónde viven sus tablas. Un error con forma de comprobado es peor que uno
  que falla. Lo encontró un agente escribiendo `migrations = "auto"`, un directorio que
  nunca existió, y axon sin decir nada. El error dice desde dónde resolvió la ruta y qué
  dejaba de comprobarse.

## [0.34.1] — 2026-09-12

### Corregido

- **`manifest_schema` describía mal lo que un agente no puede comprobar.** Las claves
  propias de un bloque salían mezcladas con los bloques anidados —así que `state` y
  `runtime` aparecían bajo `[infra.buckets.<name>]`, que es la clave de otra cosa—, la
  entrada de `[consumes]` se llamaba `x@v1` en vez de `<name>`, y los bloques que son
  mapas de campos (`[emits]`, el `in` y el `out` de un método) no salían: no tienen claves
  fijas que listar, así que el recorrido no llegaba a ellos. Ahora se nombran aparte, con
  los tipos que aceptan. Salió de usar el servidor como lo usa un agente, que es la única
  forma de ver que una respuesta es correcta pero ilegible.

## [0.34.0] — 2026-09-12

### Añadido

- **Los contratos que genera, como tercera pestaña del playground.** El panel de abajo
  pasa a tener las tres respuestas que da el compilador sobre lo que hay escrito arriba:
  lo que dicen las reglas, cómo se ve, y **qué genera**. La última es el argumento entero
  —nadie escribe a mano 450 líneas de tipos, cliente con breaker y clase base— y cambia
  mientras escribes, como las otras dos. Cambiar de archivo cambia el servicio: un
  manifiesto externo no genera nada y cae al primero que sí. Cuesta 140 KB de wasm y
  ninguna dependencia, porque `emit` ya estaba dentro. Solo se calcula la vista visible:
  generar el TypeScript en cada tecla para enseñar un diagrama es trabajo que nadie pidió.

## [0.33.0] — 2026-09-12

### Añadido

- **`axon mcp`: el compilador como herramienta que un agente puede coger.** Un modelo es
  bueno en lo que axon no hace —dónde va la frontera, cómo se llama un evento— y malo
  sabiendo si lo que acaba de escribir se sostiene, que es justo lo que `verify` contesta.
  Expone `verify`, `graph`, `manifest_schema` y `contracts` por MCP sobre stdio, sin
  dependencias nuevas. `manifest_schema` es el que evita que adivine: sale del modelo del
  propio compilador, así que lista los bloques, las claves y los valores que **esta**
  versión entiende de verdad, y una clave inventada ya es un error de carga.
  Ningún modelo corre dentro de axon: un compilador que llama a una API deja de ser
  determinista, necesita una clave y no puede ser lo que zanja una discusión.
- **`axon verify --json`.** El mismo veredicto y el mismo código de salida, para lo que
  no es una persona. La prosa se escribió para leerla una vez; esto para parsearlo.
- **`AGENTS.md`**: el bucle que funciona —un servicio, verificar, corregir, el siguiente—
  y las trampas que un modelo no puede deducir del esquema.
- **La leyenda del dibujo, y un externo y una llamada síncrona en el ejemplo.** El grafo
  tiene cuatro formas y dos flechas, y hasta ahora había que adivinarlas: caja es un
  servicio tuyo, caja redondeada uno externo, círculo un evento con el topic por el que
  viaja, flecha sólida publicar y leer, punteada una llamada síncrona —que es acoplamiento
  en el tiempo, y por eso se dibuja distinto. El ejemplo del playground creció para que
  las cuatro salgan de verdad, y sigue limpio. Lo que **no** está en el dibujo —la base,
  la cache, el índice, el pooler— la página lo dice y manda a dónde sí se ve, porque no es
  topología: es lo que cada servicio tiene dentro, no cómo se alcanzan entre ellos.
- **Dos guardianes para lo que ningún test podía ver.** La leyenda se comprueba contra lo
  que `build_graph` dibuja de verdad, en los dos sentidos: una forma explicada que el
  compilador ya no hace, o una que hace y nadie explicó, fallan. Y la página se comprueba
  entera: que su ejemplo salga limpio y que su módulo parsee.

### Corregido

- **La página se quedaba en «loading the compiler…».** Un comentario del ejemplo llevaba
  comillas invertidas dentro de un template literal de JavaScript: eso cierra el literal,
  el módulo deja de parsear y nada de la página llega a ejecutarse. No había error visible
  —solo el texto inicial— y todo lo demás seguía verde, que es exactamente por qué ahora
  hay un test que lo mira desde fuera.

## [0.32.0] — 2026-09-12

### Añadido

- **La topología, dibujada, debajo del editor del playground.** Es `axon graph` —el mismo
  mermaid, comprobado contra la salida de la terminal sobre los mismos manifiestos— y se
  redibuja mientras escribes. Que salga de ahí es el argumento entero: un diagrama de
  arquitectura dibujado a mano ya está equivocado. Y le da consecuencia visual a los
  ejercicios: al quitar el consumidor, la flecha que entraba a `notifier` desaparece y el
  evento se queda colgando en el dibujo a la vez que salta `has no consumers`. Cuesta 50
  KB comprimidos, porque `emit` tampoco toca el disco.

## [0.31.0] — 2026-09-12

### Añadido

- **El playground: el compilador corriendo en la página.** La documentación explicaba qué
  comprueba `verify` citando salidas que alguien pegó y nadie vuelve a comprobar. Ahora la
  página trae el compilador de verdad —`verify` compilado a WebAssembly, 0.61 MB con
  brotli, 15 ms sobre cinco servicios— y lo que dice es lo que diría el CI sobre los
  mismos archivos. Se aprende rompiendo: dos servicios limpios, cuatro botones que rompen
  una cosa cada uno, y la regla que salta es la lección. El editor queda libre encima.
- **El crate se puede usar sin la CLI.** `src/lib.rs` expone el núcleo —el modelo, las
  reglas— y dos features nuevas, `http` y `tui`, ambas puestas por defecto, sacan las dos
  únicas dependencias que no existen en un navegador: los sockets y la terminal. Nada del
  núcleo tocaba ninguna de las dos, que es lo que hizo esto barato.
- **`manifest::parse`**: un manifiesto desde texto, sin disco debajo. Es `load` menos el
  archivo y sus `include`.

### Cambiado

- **El binario usa la librería en vez de recompilarla.** Al aparecer `lib.rs`, `main.rs`
  seguía declarando los mismos módulos: dos compilaciones del mismo código, dos copias de
  sus pruebas, y lo que usa un solo lado pareciendo muerto en el otro. Los tres módulos
  del núcleo se reexportan y el resto del binario sigue diciendo `crate::manifest`.

### Corregido

- **`SystemTime::now` no es un reloj en un navegador: es un panic.** Lo encontró el primer
  intento de correr `verify` en wasm. La fecha la da el host, que es el único que tiene
  una, y sin ella la retirada de una versión no se puede comparar con hoy.
- **Las migraciones llegan por la misma puerta.** Sin filesystem, las reglas de esquema
  —un CRUD sobre una columna que nadie declaró, un índice sobre una tabla que no existe—
  se quedaban mudas y en su lugar salía un error sobre que no se leyó ninguna migración.
  Ahora el `.sql` entra como texto y `schemas` lo pliega igual. El aviso del `Dockerfile`
  se calla donde no hay repo que mirar: sería un error sobre la página, no sobre lo que
  alguien escribió.

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
