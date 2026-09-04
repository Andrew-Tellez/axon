# Escalado y carga

Todo esto son números, y **un número declarado que nadie comprueba es una opinión**.
axon los comprueba con aritmética:

```toml
[infra]
state           = "postgres"
ha              = true      # standby con failover. NO se lee de él
backup_retention_days = 30  # HA no es respaldo: el standby replica un DROP en segundos
pitr            = true
pool_size       = 8         # conexiones por instancia
max_connections = 200       # tope del motor
read_replicas   = 0         # de éstas SÍ se lee, y van con retraso
shard_key       = "tenant_id"
```

## La distinción que casi todos confunden

Un **standby de HA** y una **réplica de lectura** no son lo mismo, y el compilador lo
impone: del standby no se lee, existe para que el servicio siga en pie, y por eso **no
rompe la consistencia**. De una réplica de lectura sí se lee, va con retraso, y por eso
**sí la rompe** — declarar `read_replicas > 0` con `consistency = "strong"` es un error.

Y una tercera cosa que tampoco es ninguna de las dos: **HA no es respaldo.** Un standby
replica un `DROP TABLE` en segundos. Por eso un `tier = "0"` exige las dos, y con al
menos 7 días de retención: un borrado lógico se descubre después del fin de semana, no
en el minuto siguiente.

## La cuenta que nadie hace

```console
$ axon verify manifests/
error: orders: 10 conexiones x 10 instancias = 100, mas 2 reservadas, supera el tope
       de 100. El servicio se cae por agotamiento cuando escale, no cuando lo pruebes:
       baja el pool, baja max_instances, o pon un pooler delante
```

Ese error apareció sobre los ejemplos de este repo la primera vez que escribí números
realistas. **El agotamiento de conexiones no se ve con una instancia: se ve el día que
escala**, y es una multiplicación que nadie hace.

## Repartir: las reglas que nadie más impone

Con `shard_key`, `verify` bloquea cinco cosas que **no dan error, solo datos mal**. Y
esto no lo comprueba nadie: el validador de esquema de PgDog está en su roadmap sin
empezar, y Citus solo falla en tiempo de ejecución al distribuir la tabla.

| | |
| --- | --- |
| Tabla sin la clave de reparto | no se puede repartir |
| `UNIQUE` que no incluye la clave | **cada nodo la cumple por separado y el conjunto no**: dos nodos aceptan el mismo valor sin error |
| Columna `serial` / `IDENTITY` | cada nodo tiene su propia secuencia, arrancando en 1: los valores colisionan |
| `tenant_column` ≠ `shard_key` | aislar por una columna y repartir por otra hace que **toda** consulta de un inquilino toque **todos** los nodos |
| `pitr = true` + `shard_key` | N nodos son N líneas de tiempo: no existe punto de recuperación consistente para el conjunto |
| FK entre una tabla repartida y una que no | cruza nodos |

Dos excepciones, porque **una regla con falsos positivos se silencia**: una `UNIQUE`
compuesta que *sí* incluye la clave es segura —cada nodo la garantiza—, y una columna
`uuid` es única por construcción en todo el mundo, así que una PK uuid no se marca.

## El motor tiene que existir

`state` se valida contra una lista cerrada. Antes era una cadena libre, así que
`state = "neo4j"` pasaba `verify` sin un error y generaba una instancia de Cloud SQL
**Postgres**: salida incorrecta, en silencio.

```console
$ axon verify manifests/
error  g: `state = "neo4j"` no esta soportado. Motores nativos: postgres. Un motor
       distinto se resuelve con un plugin `axon-infra-neo4j`, que recibe el plan
       neutral por stdin
```

Hoy el único motor nativo es `postgres`. El plan es soportar más familias —series
temporales, grafos, columnares, documentales— y el orden natural son las **extensiones
de Postgres** (TimescaleDB, Apache AGE, pgvector), porque reusan el parser SQL, las
migraciones, la RLS y los cuatro targets que ya existen. Hasta entonces, declarar otro
motor falla y dice cómo seguir.

Y `database per service` pasó a significar una **instancia** por servicio: con todas en
la misma, un vecino ruidoso las tira juntas, así que no era aislamiento.

```sh
axon load manifests/orders.toml > carga.js
k6 run --summary-export=r.json carga.js
axon load manifests/orders.toml --check r.json
```

Los umbrales **no son números elegidos a ojo**: cada escenario corre a la tasa que
declara su `rate_limit` y falla si el p95 pasa su `timeout_ms`. Es el mismo `diff` que
`axon seq` contra `axon trace`, aplicado al rendimiento — y el script deja escrito el
techo que impone el pool declarado, para saber si el cuello es la base o el código.

Una ruta con parámetro se prueba con un id inventado, porque axon no conoce tus datos:
ahí un 404 **no** es un fallo del servicio, y el script lo dice en vez de fingir que
mide una lectura exitosa.

Y `axon build` publica las rutas del manifiesto en `rutasHttp`, para que el arranque
pueda negarse cuando falta un handler. Una ruta declarada y no servida devuelve 404 en
producción y no aparece en ninguna prueba — pasaba en el ejemplo de este repo, y lo
encontró la prueba de carga.
