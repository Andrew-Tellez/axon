-- Las fotos del agregado. Son una CACHE del fold: se pueden borrar todas y el
-- sistema sigue siendo correcto, solo mas lento. Eso es lo que las distingue
-- del flujo.
CREATE TABLE compra_snapshot (
  stream_id  uuid NOT NULL,
  version    int  NOT NULL,
  -- Con que version de las reglas se calculo. Sin esta columna, una foto vieja
  -- se rehidrata con reglas nuevas y da un estado que ya no coincide con
  -- reproducir el flujo, sin ningun error.
  reglas     int  NOT NULL,
  estado     jsonb NOT NULL,
  en         timestamptz NOT NULL DEFAULT now(),
  -- una foto por (flujo, version, reglas): reescribirla es idempotente
  PRIMARY KEY (stream_id, version, reglas)
);
