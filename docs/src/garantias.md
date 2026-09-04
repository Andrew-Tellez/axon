# Qué se comprueba

Los generadores se validan con la herramienta real del ecosistema, no con asserts
propios — un compilador que solo se verifica a sí mismo produce salida inválida:

| | |
| --- | --- |
| El TypeScript generado | `tsc --strict --noEmit` |
| El Terraform generado | `terraform validate` con los providers reales (gcp y aws), sin advertencias |
| El workflow generado | parseo YAML, bloques escalares, y que ningún target filtre otro cloud |
| El testkit generado | `node --test` contra el servicio de ejemplo real |
| El Go generado | `go vet` |
| El DDL | `PARTITION BY`, constraints de tabla, y fallo ruidoso ante SQL inválido |
| La RLS generada | se aplica a un Postgres real y se comprueba que aísla |
| Los cuatro targets | despliegan el workload y entregan a alguien |

```sh
cargo test --release      # 15 checks de conformidad
```

Preview. La superficie de comandos es estable; el formato del manifiesto todavía puede
cambiar antes de `v1`.

Saltado a propósito, y cuándo agregarlo:

- **Un generador nativo (TS)** — los demás por plugin, hasta que haya un segundo
  servicio real en otro lenguaje que justifique traerlo al core.
- **Un solo dialecto SQL (PostgreSQL)** — el esquema se lee con `sqlparser`, no con
  una regex, así que aguanta `PARTITION BY`, restricciones a nivel de tabla y lo que
  genere cualquier ORM. Un archivo que no parsea es un error, nunca un silencio.
- **Un solo modelo de ejecución (`container`)** — cualquier otro valor de `runtime` es
  un error de `verify`, no un campo ignorado en silencio. Otro modelo entra por un
  `axon-infra-*`.
- **`verify` compara declaraciones, no el cloud desplegado** — el drift contra el
  state de Terraform llega cuando haya algo desplegado que verificar.
- **Sin runtime propio** — `Bus`, `Inbox` y `Outbox` son interfaces de una línea; el
  adaptador lo pone quien despliega. Un paquete runtime cuando el mismo adaptador se
  repita en tres servicios.
