# CAP y resiliencia

El manifiesto es diseño de alto nivel — límites de servicio, topología, qué garantiza
cada uno. El compilador lo baja a diseño de bajo nivel: nivel de aislamiento, política
de reintentos, firmas de método. **Eso es todo el proyecto en una frase.**

```toml
[[depends]]
service    = "orders"
method     = "getOrder"
timeout_ms = 1000        # obligatorio
retries    = 3           # solo si el otro método es idempotente
breaker    = true

[cap]
consistency  = "strong"    # el dinero no admite un saldo viejo
on_partition = "reject"    # antes de servir algo viejo, no sirve nada
```

`axon build` emite el cliente con esa política ejecutándose: timeout, backoff
exponencial **con jitter completo** (sin jitter todos los clientes reintentan a la vez
y el otro lado nunca se levanta), y un circuito por destino que pasa a medio abierto
tras el enfriamiento. Los reintentos solo se emiten para métodos idempotentes — `verify`
bloquea el resto — y la llamada lleva `traceparent`, `x-correlation-id`,
`x-causation-id` e `idempotency-key`.

## `axon cap`: reconciliar lo declarado con lo que usás

`verify` bloquea las contradicciones. `axon cap` explica las **consecuencias**, que es
distinto: hay combinaciones que no son un error y aun así cambian lo que el servicio
puede prometer.

```console
$ axon cap manifests/ -s payments
payments  [CP]  consistency = strong, on_partition = reject
  x dependency         contradicts  `orders`, which is AP, is called on a synchronous
                                    path: the path's guarantee is the weaker one
  ! saga               costs  `refund` compensates an earlier step. A compensation is
                              eventual consistency by construction: your own state is
                              CP, the FLOW is not
  i outbox             implies  your own state stays consistent, but consumers see it
                                late: the relay publishes after the commit
  i standby HA         implies  it breaks no consistency: nobody reads from the standby,
                                it only takes over. It is the only thing on this list
                                that improves availability at no cost in the C
```

**`x` lo bloquea `verify`, `!` es un costo que pagás, `i` es una consecuencia que
conviene conocer antes de un incidente.** El filtro `-s` acota el informe, pero el
análisis sigue mirando a todos los servicios: sin `orders` cargado no se podría saber
que esa dependencia es AP.

## El lado del teorema que sí se elige

La tolerancia a particiones no es una opción: la red se parte. Lo que se elige es qué
hacer mientras está partida, y esa decisión **cambia el código**:

| | `strong` / `reject` (CP) | `eventual` / `degrade` (AP) |
| --- | --- | --- |
| `nivelAislamiento` | `SERIALIZABLE` | `READ COMMITTED` |
| Obsolescencia | — | `obsolescenciaMaximaMs`, obligatoria |
| Firma del cliente | `(input, e)` | `(input, e, respaldo)` |

La última fila es la que importa: si declarás `degrade`, el cliente generado **exige**
un parámetro `respaldo`. No podés decir "elijo disponibilidad" y después no escribir
qué se sirve cuando el otro lado no está. No es una convención — no compila.

`verify` bloquea `strong` + `degrade` (es la contradicción del teorema), exige
`max_staleness_ms` en todo `eventual` (sin un número, "eventual" es una palabra), y
avisa cuando un servicio `strong` llama sincrónicamente a uno `eventual`: **la garantía
de la ruta es la del eslabón más débil, no la tuya.**
