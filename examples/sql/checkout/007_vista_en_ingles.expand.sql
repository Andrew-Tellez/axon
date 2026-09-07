-- Los nombres que axon genera pasaron a ingles: la vista `conversion` vive en
-- `view_conversion`, y su punto lleva `view_name` y `position`. Va como
-- migracion nueva y no editando las anteriores porque ya se aplicaron: Flyway
-- compara la suma de cada archivo y una edicion falla con checksum mismatch.
ALTER TABLE vista_conversion RENAME TO view_conversion;
ALTER TABLE vista_conversion_sombra RENAME TO view_conversion_sombra;
ALTER TABLE vista_conversion_checkpoint RENAME TO view_conversion_checkpoint;
ALTER TABLE view_conversion_checkpoint RENAME COLUMN vista TO view_name;
ALTER TABLE view_conversion_checkpoint RENAME COLUMN posicion TO position;
