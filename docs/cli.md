# Referencia de comandos

Todo comando escribe a stdout y no toca el disco. `axon verify` sale con código 1
si hay errores; el resto sale 0 o falla con un mensaje en stderr.

`<fuentes>` es una lista de directorios, archivos `.toml` o URLs de servicios vivos.
Un directorio toma sus `*.toml` salvo los que empiezan con `axon.` (esos son
configuración de la herramienta). Una URL sin `.json` se resuelve a
`<url>/.well-known/axon.json`; un servicio caído se reporta y no rompe el resto.

## Código y contratos

### `axon build <manifiesto> [fuentes...] [--lang ts]`
Contratos tipados, el envelope y la clase base abstracta.

Las `fuentes` son los demás manifiestos, y hacen falta en cuanto el servicio
consume algo: **el tipo de un evento consumido lo declara su emisor**, no quien lo
recibe. Sin ellas, `build` falla diciendo qué pasar en vez de generar código que no
compila. Con `[patterns] outbox`
los emisores escriben en el outbox y `bus.publish` desaparece del archivo. Si hay
`consumes`, genera `dispatch()` con deduplicación por id.

Además del contrato emite: la tabla de transiciones de cada `[machine]`, el
`nivelAislamiento` que sale del lado CAP declarado, los clientes de cada `[[depends]]`
con su política ejecutándose (timeout, backoff con jitter, circuito), y —si hay campos
`pii`— la lista y un `redactar()` recursivo.

`--lang` distinto de `ts` busca `axon-gen-<lang>` en el `PATH` y le pasa
`{"manifest": ..., "peers": [...]}` por stdin. `plugins/axon-gen-go` es el
generador de referencia.

### `axon test <manifiesto> <fuentes> [--lang ts] [--contracts ./contracts.ts]`
Un testkit que **compila por sí solo**: dobles en memoria de `Bus`, `Inbox` y
`Outbox`, fixtures derivadas del esquema del **emisor** de cada evento, y dos suites
exportadas.

No adivina dónde vive tu código: `pruebasDeContrato` recibe una fábrica. Tejerlo son
tres líneas escritas a mano:

```ts
import { pruebasDeContrato, pruebasDeMaquinas } from "./axon.testkit.ts";
import { Payments } from "./index.ts";
pruebasDeContrato((bus, inbox, outbox) => new Payments(bus, inbox, outbox, db));
pruebasDeMaquinas();
```

`pruebasDeContrato` comprueba lo que el manifiesto promete: que el handler acepta el
evento tal como lo emite su dueño, que la segunda entrega del mismo id no repite el
efecto, que la cadena causal (`causationId`, `correlationId`, `traceparent`) sobrevive
al handler, y que con `outbox` declarado nada se publica directo al bus.
`pruebasDeMaquinas` no necesita tu código: recorre la tabla de transiciones y verifica
que cada una sea legal desde sus orígenes e ilegal desde cualquier otro estado.

Corre con `node --test`, sin dependencias.

### `axon openapi <fuentes>`
OpenAPI 3.1 de toda la plataforma en un documento. `Idempotency-Key` obligatorio en
métodos mutantes y `application/problem+json` (RFC 7807) como error uniforme.

### `axon discover <fuentes>`
Registro JSON: versión, dueño, métodos con entradas y salidas, eventos emitidos y
consumidos. Funciona contra disco y contra servicios corriendo.

### `axon import asyncapi <archivo|-> [--service <nombre>]`
AsyncAPI 2.x o 3.x, JSON o YAML, a un manifiesto en stdout. El nombre del servicio
sale de `info.title` salvo que se pase `--service`.

En 2.x la dirección es desde fuera de la app: `publish` es lo que otros publican
hacia ella (lo que la app **consume**) y `subscribe` lo que expone para que otros
lean (lo que **emite**). axon lo traduce; es la confusión número uno al leer 2.x.

Lo que AsyncAPI no declara sale como `TODO` y `verify` lo trata como ausente.

### `axon rls <fuentes> [--target sql|pg_anon]`
Políticas de acceso a datos: RLS por fila y vistas enmascaradas por columna. Sale del
cruce del esquema real (leído de las migraciones) con los campos `pii` del manifiesto.

La salida es **una migración más** — guardala como `sql/<servicio>/090_rls.expand.sql`
y aplicala con tu runner. No es un comando que se corre a mano: el esquema lo gobiernan
las migraciones.

Para que la política haga algo, la aplicación tiene que conectarse con un rol **no
superusuario** (un superusuario salta RLS siempre) y fijar el inquilino en la sesión:

```sql
SET axon.tenant = '<uuid del inquilino>';
```

Con `--target pg_anon` emite el diccionario sensible de
[pg_anon](https://github.com/TantorLabs/pg_anon) en vez del SQL: eso sirve para hacer
una **copia** enmascarada (staging, soporte, terceros), no para proteger la consulta
viva. La regla sale del tipo de la columna, y las tablas del framework se excluyen del
dump porque el `outbox` lleva payloads.

## Infraestructura

### `axon infra <fuentes> [--target local|gcp|aws|k8s|plan] [--env <nombre>]`
Produce el plan neutral y lo renderiza. El plan cubre el edge (rutas, auth, rate
limit, timeouts), la mensajería (topics, suscripciones, DLQ), el cómputo, el estado,
los buckets con su CDN, los secretos y las variables de OpenTelemetry.

Los atributos de recurso de OTel salen del manifiesto (`owner`, `tier`, `version`) y el
muestreo del `tier`: tier 0 se traza entero. `local` levanta el backend y traza todo sin
importar el tier; los demás targets exportan al endpoint que declares. `--env` aplica los deltas de `[env.<nombre>]`
sobre `[infra]`. Un `--target` no nativo busca `axon-infra-<target>` en el `PATH`.

`--target plan` imprime el plan en JSON: la salida de emergencia para cualquier
proveedor sin target.

### `axon ci <manifiesto> [--target gcp|aws|k8s]`
Pipeline de GitHub Actions. Los **gates** los sabe axon: `verify` contra todos los
manifiestos (no solo el propio), código generado al día, migraciones en dry-run con
la convención de nombres correcta, OIDC en vez de llaves, e infra aplicada antes que
código — el topic tiene que existir cuando arranque el primer pod que publica en él.

El **despliegue** sale del `--target`, igual que la infraestructura: Cloud Run, ECS o
`kubectl rollout`. Sin `--target` genera solo los gates y falla en el paso de deploy
con una nota: esa parte la sabe tu equipo, no axon.

El **layout del repo** sale de `[ci]` en `axon.policy.toml`, donde `{service}` se
sustituye por el nombre del servicio:

```toml
[ci]
manifests_dir  = "manifests"
service_dir    = "services/{service}"
test_cmd       = "make -C services/{service} test"
contracts_path = "services/{service}/src/contracts.ts"
image          = "${{ vars.REGISTRY }}/{service}:${{ github.sha }}"
```

## Diagramas

| Comando | Sale de | Da |
| --- | --- | --- |
| `axon graph <fuentes>` | manifiestos | topología de eventos |
| `axon classes <fuentes>` | manifiestos | diagrama de clases |
| `axon er <fuentes>` | migraciones | entidad-relación |
| `axon states <fuentes>` | `[machine.*]` | máquinas de estado del dominio |
| `axon seq <evento> <fuentes>` | cadena causal declarada | secuencia esperada |

Todos emiten Mermaid. GitHub los renderiza en un bloque ` ```mermaid `.

El esquema para `er` y para el chequeo de FK entre servicios se lee con un parser
SQL de PostgreSQL, no con expresiones regulares. Un archivo que no parsea aborta el
comando con el archivo y el error: axon prefiere fallar a adivinar columnas.

## Debug

### `axon trace [log] [--correlation <id>] [--seq]`
Lee NDJSON de envelopes (`-` o nada = stdin) y reconstruye la cadena causal real.
Sin `--seq` imprime el árbol; con `--seq`, Mermaid para diffear contra `axon seq`.

## Verificación

### `axon baseline <fuentes>`
Snapshot JSON de los contratos publicados: esquema de cada evento con su dueño, y la
firma de cada método. Guardalo como `axon.baseline.json` junto a los manifiestos y
commiteálo. Se regenera **al publicar**, no en cada cambio.

### `axon verify <fuentes>`
Todas las reglas del README, más `axon.policy.toml` si existe junto a la primera
fuente, más `axon.baseline.json` si está, más todos los `axon-check-*` del `PATH`.
Sale con 1 si hay errores.

Sin baseline no puede detectar un cambio incompatible en una versión ya publicada, y
lo dice como aviso en vez de callarse.
