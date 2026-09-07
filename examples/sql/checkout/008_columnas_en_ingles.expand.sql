-- Las columnas que axon EXIGE pasan a ingles: el diario de la saga lleva
-- `step`, `status`, `data` y `updated`, y la foto lleva `state` y `rules`.
-- Renombrar y no agregar-y-copiar porque estas columnas son de axon, no del
-- dominio: no hay dos escritores que puedan discrepar mientras se migra.
ALTER TABLE saga_compra RENAME COLUMN paso TO step;
ALTER TABLE saga_compra RENAME COLUMN estado TO status;
ALTER TABLE saga_compra RENAME COLUMN datos TO data;
ALTER TABLE saga_compra RENAME COLUMN actualizado TO updated;
ALTER TABLE compra_snapshot RENAME COLUMN estado TO state;
ALTER TABLE compra_snapshot RENAME COLUMN reglas TO rules;
ALTER TABLE saga_compra RENAME COLUMN salidas TO outputs;
