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

## El pooler y el reparto

```toml
[pooler]
engine          = "pgdog"
mode            = "transaction"
tenant_binding  = "set_local"   # obligatorio en `transaction` con inquilinos
shards          = 4
max_client_conn = 500
pool_size       = 40
```

```sh
axon pooler manifests/ > pgdog.toml
```

Sale la configuración de [pgdog](https://pgdog.dev) con los nodos, y **el reparto
derivado del esquema real**: qué tablas llevan la clave y de qué tipo es, leído de las
migraciones. Los datos de conexión salen como variables — un archivo generado no es
lugar para una contraseña.

Dos decisiones que el generador toma solo, y por qué:

- **`cross_shard_disabled = true`.** Sin `JOIN` entre nodos ni unicidad global, una
  consulta que cruza se responde *mal*. Rechazarla convierte cada limitación del sharder
  en un error ruidoso.
- **`query_parser = "on"`, no `"auto"`.** En `auto` el parser no se activa con un solo
  nodo primario — que es exactamente el caso donde una variable de sesión se cuela sin
  ser interceptada. Y el síntoma **desaparece al agregar una réplica**, así que no se
  reproduce en un staging que la tenga.

### La regla que importa

```console
$ axon verify manifests/
error  p: `mode = "transaction"` con `tenant_column` y sin `tenant_binding = "set_local"`.
       La conexion vuelve al pool en cada COMMIT y se le entrega a otro inquilino: una
       GUC de sesion sobrevive y la siguiente peticion lee las filas del anterior, sin
       un error. `SET LOCAL` muere con la transaccion
```

En modo transacción la conexión física se recicla entre inquilinos. Si el inquilino se
fija con un `SET` de sesión, el valor sobrevive y **la siguiente petición lee las filas
del anterior**. Eso no falla: devuelve datos del cliente equivocado.

Las demás reglas del pooler cambian el **sujeto** de la aritmética, que es la parte que
se pasa por alto:

| | |
| --- | --- |
| `pool_size` × `max_instances` > `max_client_conn` | con un pooler en medio, las instancias compiten por *sus* clientes, no por las conexiones del motor |
| `pooler.pool_size` + reservadas > `max_connections` | **por motor**: los nodos y las réplicas son motores distintos, cada uno con su tope |
| `mode = "session"` con más clientes que conexiones | en modo sesión no hay multiplexado: una conexión de cliente ata una de servidor |
| `shards > 1` con `consistency = "strong"` | la confirmación en dos fases deja ver estados parciales: la garantía real es eventual |
| `shards > 1` sin `shard_key` | el sharder necesita la columna, y `verify` necesita comprobar que toda tabla la lleve |
| Campos de `[pooler]` con `engine = "none"` | configuración que no se aplica en ninguna parte |

### Verificado contra su propio esquema

pgdog publica el **JSON Schema oficial** de su configuración, generado desde sus tipos
de Rust y comprobado por su CI. El suite valida el `pgdog.toml` generado contra ese
archivo: eso es validar contra el parser real que lo va a leer, no contra nuestra idea de
cómo debería ser.

### Lo que falta, y es lo que decide

Wirear pgdog al target `local`, delante de N Postgres, y **comprobar en el demo que la
RLS generada sigue aislando a través del pooler en modo transacción**. Hoy eso está
declarado y verificado en el manifiesto, pero no medido de punta a punta.

Es el experimento que importa, porque el único test de RLS multi-inquilino que existe en
el repositorio de pgdog es el que un contribuyente externo escribió para demostrar una
fuga de `set_config()`, y está en rojo mientras su corrección no esté integrada.

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
