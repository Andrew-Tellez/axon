# Fixtures

## `pgdog.schema.json`

El JSON Schema **oficial** de `pgdog.toml`, tomado de
`pgdogdev/pgdog@main:.schema/pgdog.schema.json`.

Lo generan desde sus propios tipos de Rust y su CI falla si se desincroniza, así que
validar contra este archivo es validar contra el parser real de pgdog — no contra una
idea nuestra de cómo debería ser su configuración.

Está fijado a propósito. Si pgdog cambia su esquema, el test sigue comprobando contra
esta versión: actualizarlo es un acto deliberado, igual que el `axon.baseline.json` de
los contratos. La alternativa —bajarlo en cada corrida— haría que un test se pusiera
rojo sin que cambie ningún input nuestro.
