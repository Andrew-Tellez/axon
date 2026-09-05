# Bodega y métricas de negocio

Esto es **derivable entero**: axon ya conoce el esquema de cada evento, qué campos son
personales, y —lo más importante— **quién causa a quién**. Esa última parte es la que
ninguna bodega tiene.

```toml
[analytics]
export    = true
pii       = "hash"        # o "exclude", que es el default
warehouse = "clickhouse"  # bigquery · snowflake · clickhouse
```

```sh
axon analytics manifests/ --target clickhouse > bodega.sql
```

## La bodega se declara, y por eso hay ingesta

`warehouse` estaba como bandera de la CLI, y eso permitía generar el esquema de Snowflake
y desplegar una infraestructura que **no lleva nada ahí**: el esquema se aplicaba, las
tablas se quedaban vacías, y nadie veía un error. Una bodega vacía es indistinguible de
«no pasó nada en el negocio».

Declarada, `axon infra` puede cablear la ingesta — o negarse:

```console
$ axon infra manifests/ --target gcp
error  `[analytics] warehouse = "clickhouse"` no tiene camino de ingesta en `gcp`. El
       esquema se genera igual y las tablas se quedarian vacias sin un solo error.
       Combinaciones cableadas: gcp+bigquery, aws+snowflake, aws+clickhouse,
       local+clickhouse. O `export = false` si este entorno no exporta.
```

| target + bodega | qué se despliega |
| --- | --- |
| `gcp` + `bigquery` | una suscripción de Pub/Sub que escribe **directo** a la tabla, con `use_table_schema` y su propia DLQ |
| `aws` + `snowflake` o `clickhouse` | un Firehose por evento hacia S3, con el **mismo particionado por fecha** que el esquema generado. De ahí la bodega carga con lo suyo —Snowpipe, una tabla externa— porque ese paso vive del lado de la bodega, no del proveedor |
| `local` + `clickhouse` | un ClickHouse y un cargador del log de envelopes que el propio target ya escribe |
| `k8s` + cualquiera | **nada, y lo dice**. Un clúster no trae bodega: apuntarías a la que corras, y axon no puede adivinarla |

Y una bodega por plataforma: `verify` exige que todos los servicios que exportan declaren
la misma. Repartidos entre dos, el embudo —que es lo que hace útil exportar— no se puede
armar con una consulta, y cada tabla existiría con filas sin que nada avise.

### Por qué el Firehose lleva un `error_output_prefix`

Lo que no encaja en el esquema cae en `errores/` y se puede volver a cargar. Es el
equivalente del DLQ para la bodega: un evento que se descarta en silencio es un agujero en
el histórico que nadie va a notar hasta que alguien pregunte por un número que no cuadra.

## Una tabla por evento

```sql
CREATE TABLE IF NOT EXISTS `@dataset.order_placed_v1` (
  event_id STRING NOT NULL,
  event_type STRING NOT NULL,
  source STRING NOT NULL,
  event_time TIMESTAMP NOT NULL,
  trace_id STRING,
  correlation_id STRING NOT NULL,
  causation_id STRING,
  order_id STRING,
  customer_id STRING,
  total_amount INT64,
  total_currency STRING
)
PARTITION BY DATE(event_time)
CLUSTER BY correlation_id, source;
```

Tres decisiones que no son de estilo:

- **Las columnas del envelope van siempre.** Sin `correlation_id` no hay embudo posible,
  y sin `causation_id` no se puede reconstruir qué disparó qué.
- **Particionar no es opcional.** Sin `PARTITION BY DATE(event_time)`, cada consulta
  escanea la tabla entera y la factura crece con el histórico.
- **`money` se aplana en dos columnas.** Sumar un objeto no se puede, y un importe que
  no se puede sumar no sirve en una bodega. Los nombres pasan a `snake_case`: los
  contratos usan la convención del lenguaje, la bodega usa la suya.

## Los datos personales, por decisión explícita

```toml
pii = ["customerEmail"]

[analytics]
pii = "hash"        # o "exclude", que es el default
```

**El default es no exportarlos.** Una bodega es el lugar donde un dato personal vive más
tiempo, se copia más veces y lo lee más gente, así que el valor seguro tiene que ser el
que no lo manda.

Con `pii = "hash"` se exporta un SHA-256 con salt, para poder contar clientes únicos sin
guardar el correo. Y `verify` avisa de lo que eso sigue siendo:

```console
aviso  orders: exporta 2 campos personales hasheados a la bodega. Un hash no es
       anonimizacion: identifica a la misma persona entre tablas, asi que sirve
       para contar y tambien para cruzar
```

## El embudo sale de la cadena declarada

Esto es lo que ninguna otra herramienta puede hacer: **un embudo normalmente se arma
adivinando cómo se relacionan los eventos.** Acá está escrito en el manifiesto, así que
la vista se deriva.

```sql
CREATE OR REPLACE VIEW `@dataset.embudo_order_placed_v1` AS
SELECT
  correlation_id,
  MIN(IF(event_type = 'order.placed@v1',     event_time, NULL)) AS paso_1_order_placed_v1,
  MIN(IF(event_type = 'payment.captured@v1', event_time, NULL)) AS paso_2_payment_captured_v1,
  TIMESTAMP_DIFF(
    MIN(IF(event_type = 'payment.captured@v1', event_time, NULL)),
    MIN(IF(event_type = 'order.placed@v1',     event_time, NULL)),
    MILLISECOND
  ) AS ms_hasta_payment_captured_v1
FROM (
    SELECT correlation_id, event_type, event_time FROM `@dataset.order_placed_v1`
    UNION ALL
    SELECT correlation_id, event_type, event_time FROM `@dataset.payment_captured_v1`
)
GROUP BY correlation_id;
```

Una fila por flujo de negocio. **Un paso en `NULL` es un flujo que no llegó ahí: eso es
la conversión.** Y el `TIMESTAMP_DIFF` es la latencia *de negocio* — cuánto tarda un
pedido en cobrarse, no cuánto tarda una petición HTTP.

Los pasos son los mismos que dibuja [`axon seq`](./cli.md), porque salen de la misma
declaración. Si mañana agregás un consumidor a la cadena, el embudo gana un paso sin que
nadie edite el SQL.

## El sink, sin proceso intermedio

El target `gcp` emite la suscripción que escribe **directo** en BigQuery:

```hcl
resource "google_pubsub_subscription" "order_placed_v1_bodega" {
  topic = google_pubsub_topic.order_placed_v1.name
  bigquery_config {
    table            = "${var.project}.${var.dataset}.order_placed_v1"
    use_table_schema = true
    write_metadata   = true
  }
  dead_letter_policy { ... }
}
```

No hay un proceso que mantener, ni uno más donde el evento pueda perderse. Y **la bodega
también lleva DLQ**: un mensaje que no encaja en el esquema no puede desaparecer en
silencio — que es justo lo que pasa cuando alguien cambia un campo y nadie mira la
tabla.

## Tres bodegas, un esquema

```sh
axon analytics manifests/ --target bigquery|snowflake|clickhouse|plan
```

El esquema y los embudos son **los mismos** —salen del mismo manifiesto—; lo que cambia
es el dialecto. Y las diferencias no son cosméticas:

| | BigQuery | Snowflake | ClickHouse |
| --- | --- | --- | --- |
| Cadena | `STRING` | `VARCHAR` | `Nullable(String)` |
| Entero | `INT64` | `NUMBER(38,0)` | `Nullable(Int64)` |
| Marca de tiempo | `TIMESTAMP` | `TIMESTAMP_TZ` | `Nullable(DateTime64(3))` |
| JSON | `JSON` | `VARIANT` | `String` |
| Particionado | `PARTITION BY DATE(...)` | automático | `PARTITION BY toYYYYMM(...)` |
| Agrupado | `CLUSTER BY` | `CLUSTER BY (...)` | `ORDER BY (...)` en `MergeTree` |
| Latencia | `TIMESTAMP_DIFF` | `TIMESTAMPDIFF` | `dateDiff` |

- En **Snowflake** declarar `PARTITION BY` sería un error, no una optimización: sus
  micro-particiones son automáticas.
- En **ClickHouse** la nulabilidad va en el tipo, y el `ORDER BY` de `MergeTree` decide
  qué consultas son rápidas — va primero el flujo, porque un embudo agrupa por él.
- Los embudos usan `CASE WHEN` y no `IF()`/`IFF()`: es lo único que las tres entienden
  igual.

Para cualquier otra, `--target plan` da el plan neutral en JSON —tablas, columnas, tipos
de axon, partición, clustering y el modo de PII— y lo renderizás vos.

## Verificado

El suite **parsea el DDL de cada bodega con su propio dialecto** de `sqlparser`, no lo
compara contra un texto esperado. Comparar contra texto no habría encontrado ninguno de
los tres bugs que esto encontró:

- Un comentario al final de una columna **se come la coma** que la separa de la siguiente
  — el mismo error que ya había cometido en una migración.
- Desenvolver `Nullable(DateTime64(3))` quitando todos los paréntesis del final dejaba
  `DateTime64(3,` y rompía el tipo parametrizado.
- ClickHouse espera `ORDER BY` justo después del motor: con `PARTITION BY` en medio, el
  DDL no parsea.

## Medido en el demo, contra ClickHouse

El target local levanta la bodega y la llena del log de envelopes. Eso no es un artefacto
del demo: `AXON_TRACE_LOG` está en el compose generado, así que **la traza y la analítica
salen de la misma fuente** — y el `trace_id` de cada fila sale del `traceparent` del
envelope, así que desde un embudo se puede saltar a la traza de ese flujo concreto.

```console
==> la bodega: esquema, embudo y PII
  OK: 12 eventos en la bodega, con el esquema generado sin tocarlo
  OK: 12 flujos, 12 llegaron al cobro (conversion 100%)
  i latencia de negocio del embudo: 15ms de la orden al cobro
  OK: 12 hasheados, 0 correos en claro
  OK: 12 filas antes y despues; el cargador es idempotente
```

Cinco cosas, cada una una pregunta distinta:

| | |
| --- | --- |
| **el esquema generado acepta los eventos reales** | se aplica sin tocarlo. Si una columna o un tipo no cuadrara, la carga fallaría aquí y no en producción |
| **el embudo declarado cuenta el flujo que ocurrió** | y los flujos del embudo coinciden con las filas del paso 1: si no, estaría contando flujos de otra parte |
| **la latencia de negocio es un número** | del `order.placed@v1` al `payment.captured@v1`, no la de una petición |
| **el campo personal no viaja en claro** | `pii = "hash"` declarado → 64 caracteres hex, 0 arrobas. Que la política se aplicó no se sabe leyendo el manifiesto |
| **cargar dos veces no duplica** | un cargador periódico corre muchas veces sobre el mismo log; sin filtrar por lo ya cargado, cada evento se multiplica y el embudo miente sin fallar |

El cargador sale del mismo sitio que el esquema —las columnas y sus rutas dentro del JSON—
así que no pueden desincronizarse. Y el salt del hash entra por parámetro, nunca en el SQL
generado.

## Lo que falta

`k8s` no tiene camino de ingesta, y el rechazo lo dice en vez de fingirlo. Cerrarlo pide un
consumidor del broker que escriba en la bodega —lo que en local se resuelve leyendo el log,
en un clúster hay que consumir— y eso es código, no IaC.

Tampoco hay retención declarable de las tablas de la bodega, ni un `axon analytics --check`
que compare el esquema declarado contra el que existe allí.
