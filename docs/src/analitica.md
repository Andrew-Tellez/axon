# Bodega y métricas de negocio

Esto es **derivable entero**: axon ya conoce el esquema de cada evento, qué campos son
personales, y —lo más importante— **quién causa a quién**. Esa última parte es la que
ninguna bodega tiene.

```sh
axon analytics manifests/ --target bigquery > bodega.sql
```

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

## Para otra bodega

```sh
axon analytics manifests/ --target plan
```

El plan neutral en JSON —tablas, columnas, tipos, partición, clustering y el modo de
PII— para renderizarlo a Snowflake, Redshift, ClickHouse o lo que uses.

## Verificado

El suite **parsea el DDL generado con el dialecto de BigQuery de `sqlparser`**, no lo
compara contra un texto esperado. Eso encontró un bug real: un comentario al final de una
columna se come la coma que la separa de la siguiente, y el DDL queda inválido — el mismo
error que ya había cometido en una migración.
