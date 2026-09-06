-- El punto de la vista es POR FLUJO. La version de un evento es su posicion
-- dentro de su flujo, asi que un solo numero para toda la vista no identifica
-- nada en cuanto hay mas de un flujo: con uno parecia funcionar, y el fallo
-- aparecio al reconstruir sobre varios.
ALTER TABLE vista_conversion_checkpoint ADD COLUMN stream_id uuid;
DELETE FROM vista_conversion_checkpoint;
ALTER TABLE vista_conversion_checkpoint DROP CONSTRAINT vista_conversion_checkpoint_pkey;
ALTER TABLE vista_conversion_checkpoint ALTER COLUMN stream_id SET NOT NULL;
ALTER TABLE vista_conversion_checkpoint ADD PRIMARY KEY (vista, stream_id);
