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

`--lang` distinto de `ts` busca `axon-gen-<lang>` en el `PATH`.

### `axon test <manifiesto> <fuentes> [--lang ts]`
Andamiaje de las tres capas. Las fixtures se derivan del esquema del **emisor**, no
de lo que el consumidor cree que recibe — ahí es donde aparece el drift. Incluye por
defecto el test de idempotencia (dos entregas del mismo id, un solo efecto) y un e2e
que compara `axon trace --seq` contra `axon seq`.

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

## Infraestructura

### `axon infra <fuentes> [--target local|gcp|aws|k8s|plan] [--env <nombre>]`
Produce el plan neutral y lo renderiza. `--env` aplica los deltas de `[env.<nombre>]`
sobre `[infra]`. Un `--target` no nativo busca `axon-infra-<target>` en el `PATH`.

`--target plan` imprime el plan en JSON: la salida de emergencia para cualquier
proveedor sin target.

### `axon ci <manifiesto>`
Pipeline de GitHub Actions con los gates que importan: `verify` contra **todos** los
manifiestos (no solo el propio), código generado al día, migraciones en dry-run,
OIDC en vez de llaves, infra antes que código, y un `verify` final contra la URL
desplegada.

## Diagramas

| Comando | Sale de | Da |
| --- | --- | --- |
| `axon graph <fuentes>` | manifiestos | topología de eventos |
| `axon classes <fuentes>` | manifiestos | diagrama de clases |
| `axon er <fuentes>` | migraciones | entidad-relación |
| `axon states <fuentes>` | `[machine.*]` | máquinas de estado del dominio |
| `axon seq <evento> <fuentes>` | cadena causal declarada | secuencia esperada |

Todos emiten Mermaid. GitHub los renderiza en un bloque ` ```mermaid `.

## Debug

### `axon trace [log] [--correlation <id>] [--seq]`
Lee NDJSON de envelopes (`-` o nada = stdin) y reconstruye la cadena causal real.
Sin `--seq` imprime el árbol; con `--seq`, Mermaid para diffear contra `axon seq`.

## Verificación

### `axon verify <fuentes>`
Todas las reglas del README, más `axon.policy.toml` si existe junto a la primera
fuente, más todos los `axon-check-*` del `PATH`. Sale con 1 si hay errores.
