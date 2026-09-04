# Seguridad

Cada regla cita su categoría del [OWASP Top 10 (2021)](https://owasp.org/Top10/),
porque un error que no dice *por qué* importa se silencia con un allow:

| | Regla | |
| --- | --- | --- |
| **A01** | Ruta pública que muta en un servicio `tier = "0"` | error |
| **A01** | Tabla sin la columna del inquilino cuando hay `tenant_column` | error |
| **A02** | Un secreto literal donde va su nombre | error |
| **A04** | Ruta pública sin `timeout_ms` | error |
| **A05** | Bucket público sin `retention_days` | aviso |
| **A08** | `[ci].image` sin digest — una etiqueta es mutable | aviso |
| **A09** | Un campo declarado `pii` devuelto por una ruta pública | error |

Y lo que no se avisa, se **genera endurecido**:

- **A05** — el `Deployment` de k8s sale con `runAsNonRoot`, `readOnlyRootFilesystem`,
  `capabilities: drop ALL`, `seccompProfile: RuntimeDefault` y sin token de service
  account montado. Más una `NetworkPolicy` de denegación por defecto: al pod solo entra
  el edge.
- **A01** — un servicio sin ninguna ruta pública se despliega con
  `ingress = INTERNAL_LOAD_BALANCER`: no tiene puerta a internet aunque alguien se
  equivoque en el gateway.
- **A08** — el pipeline construye la imagen, toma su digest y **despliega por digest**,
  con `provenance: true`.
- **A09** — `axon build` emite `camposPII` y un `redactar()` recursivo. Un dato personal
  se filtra por un log, no por un exploit.

## RLS y enmascarado

```toml
pii = ["customer_email"]

[infra]
tenant_column = "tenant_id"
tenant_exempt = ["auditoria"]   # lo que no es de negocio
```

`axon rls` cruza dos cosas que axon ya sabe — el esquema real (leído de las migraciones
con un parser SQL) y los campos `pii` — y emite una migración más:

```sql
ALTER TABLE "order" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "order" FORCE ROW LEVEL SECURITY;   -- también al dueño de la tabla
CREATE POLICY "order_inquilino" ON "order"
  USING ("tenant_id" = current_setting('axon.tenant', true)::uuid)
  WITH CHECK ("tenant_id" = current_setting('axon.tenant', true)::uuid);

CREATE OR REPLACE VIEW "order_enmascarada" AS SELECT
  id, customer_id, total_cents, status, tenant_id,
  '[redactado]'::text AS "customer_email"
FROM "order";
REVOKE ALL ON "order" FROM axon_lectura;
GRANT SELECT ON "order_enmascarada" TO axon_lectura;
```

La regla de `verify` es la que importa: **una tabla que se olvida de la columna del
inquilino no recibe política, y una tabla sin política no falla — devuelve las filas de
todos.** Ese es el modo de fallo silencioso que la declaración elimina.

El suite no lee ese SQL: lo aplica a un Postgres real y comprueba que sin inquilino se
ven 0 filas, que cada inquilino ve solo la suya, que escribir para otro se rechaza, y
que la vista devuelve `[redactado]`.

## Una copia enmascarada, con `pg_anon`

Las vistas protegen la **consulta viva**: un rol de analítica nunca ve el dato crudo.
Para el otro problema —darle datos realistas a staging, a soporte o a un tercero que no
debe ver los de verdad— hace falta una **copia** enmascarada, y eso lo hace
[`pg_anon`](https://github.com/TantorLabs/pg_anon) (que no es una extensión de Postgres,
sino un CLI que clona la base reemplazando los campos en el camino).

Su mayor friccón es mantener el diccionario de campos sensibles, y su `create-dict` lo
detecta **por heurística**. axon lo emite declarado:

```sh
axon rls manifests/ --target pg_anon > sens_dict.py
pg_anon --mode=dump --prepared-sens-dict-file=sens_dict.py ...
```

La regla sale del **tipo** de cada columna, nunca de su nombre: adivinar porque una
columna se llama `email` falla con `correo` y se equivoca con `email_template`. Lo que
axon garantiza es la **cobertura** —ningún campo declarado `pii` se queda sin regla—; la
regla en sí es tuya y se edita, por ejemplo para preservar la forma de un correo.

Las tablas del propio framework se excluyen del dump: el `outbox` lleva el payload de
cada evento en un `jsonb`, que es el último lugar donde uno buscaría una fuga.

El suite aplica cada regla generada a una columna de su tipo en un Postgres real —un
cast que falta hace fallar el dump a mitad de camino— y comprueba que el dato original
no sobreviva a ninguna.
