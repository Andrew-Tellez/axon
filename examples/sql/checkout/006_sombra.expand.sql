-- La sombra de la vista: misma forma, se construye aparte y se cambia por la
-- viva de golpe. Reconstruir en el sitio deja la vista incompleta mientras
-- corre, y se sigue leyendo: los que preguntan reciben menos filas de las que
-- hay, sin ningun error.
CREATE TABLE vista_conversion_sombra (
  stream_id   uuid PRIMARY KEY,
  estado      text NOT NULL,
  centavos    bigint,
  payment_id  uuid,
  motivo      text,
  evento_en   timestamptz NOT NULL
);
