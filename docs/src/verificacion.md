# Reglas y drift

Lo que convierte al manifiesto en algo más que documentación.

```console
$ axon verify manifests/
error  billing consume payment.refunded@v1 pero nadie lo emite
error  payments.payment.stuck no es final y no tiene salida; es un deadlock
error  orders.payment.order_id -> payment: FK cruza el limite de servicio
aviso  payments es strong y llama a orders que es eventual
falla  4 servicios, 3 errores, 1 avisos
```

Sale con código 1 si hay errores. Los errores van primero: en una lista larga, lo que
hay que arreglar no puede quedar debajo.

## Contratos

| | |
| --- | --- |
| Se consume un evento que nadie emite | error |
| Dos emisores del mismo evento con esquemas distintos | error |
| Se llama un método que el otro servicio no expone | error |
| Un evento se emite y nadie lo consume | aviso |

## Resiliencia

| | |
| --- | --- |
| Dependencia sin `timeout_ms` | error |
| Reintentos sobre un método que no se declara idempotente | error |
| Reintentos sin `breaker` | aviso |
| Más dependencias síncronas que el límite de la policy | aviso |

## API y edge

| | |
| --- | --- |
| Ruta HTTP sin versión, o duplicada entre servicios | error |
| Método mutante sin `idempotent` | error |
| Ruta expuesta sin `auth` | error |
| Ruta pública sin `rate_limit` o sin `timeout_ms` | error |
| Método paginado que no devuelve `cursor` | error |

## Datos

| | |
| --- | --- |
| FK que cruza el límite de un servicio | error |
| Tabla sin la columna del inquilino cuando hay `tenant_column` | error |
| Tabla sin la clave de reparto cuando hay `shard_key` | error |
| FK entre una tabla repartida y una que no lo está | error |
| Migración destructiva sin `.contract.sql` | error |
| Migración sin prefijo numérico | aviso |

## Máquinas de estado

| | |
| --- | --- |
| Estado inalcanzable desde el inicial | error |
| Estado no final sin salida — un deadlock | error |
| Transición disparada por un método o evento que no existe | error |
| Transición que emite un evento que el servicio no declara | error |
| Compensación que apunta a un paso inexistente | error |

## Escalado

| | |
| --- | --- |
| `pool_size` × `max_instances` supera `max_connections` | error |
| Réplicas de lectura bajo una promesa `strong` | error |
| `tier = "0"` sin `ha` o sin `backup_retention_days` | error |
| `pitr` sin respaldos | error |

## Feature flags

| | |
| --- | --- |
| Flag sin `owner` | error |
| Flag sin `expires` que no sea `kill_switch` | error |
| Flag vencido | error |
| Rollout parcial sin `sticky_by` | error |
| `kill_switch` con `rollout` | error |
| `sticky_by` por un campo que no aparece en ningún contrato | error |

## Una versión publicada es inmutable

`verify` compara los manifiestos entre sí, pero eso no alcanza para el error más común
y más caro: **cambiarle un campo a una versión que ya está en producción**, con
consumidores desplegados esperándola como estaba. Para verlo hace falta un registro de
lo que se publicó.

```console
$ axon baseline manifests/ > manifests/axon.baseline.json   # al publicar
$ axon verify manifests/
error  order.placed@v1.total: cambio de `money` a `int` en una version publicada;
       publica `order.placed@v2` en su lugar
```

| | |
| --- | --- |
| Un campo cambia de tipo en una versión publicada | error |
| Un campo aparece o desaparece en una versión publicada | error |
| Un evento publicado deja de emitirse, o cambia de dueño | error |
| Una ruta HTTP se mueve | error |
| Un método deja de devolver un campo, o exige uno nuevo | error |
| Contratos nuevos aún sin registrar | aviso |

En axon **todo campo es obligatorio**, así que agregar uno rompe igual que quitarlo: no
hay campos opcionales que hagan un cambio «compatible».

Si de verdad querés retirar una versión, la quitás del `axon.baseline.json` en el mismo
PR. **Ese diff es la vía de escape**, y se revisa como cualquier otro cambio — no hay un
`--force` que se pueda copiar sin pensar.

Sin el archivo, `verify` avisa que no puede ver esta clase de problema, en vez de
callarse.

## Reglas propias

Todo `axon-check-*` en el `PATH` corre siempre y bloquea igual que una regla nativa.
Ver [Plugins](./plugins.md).
