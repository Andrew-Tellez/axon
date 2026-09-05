-- expand: el registro de intentos. Existe para poder CONTARLOS: la politica de
-- reintentos que declara el manifiesto no se puede comprobar sin saber cuantas
-- veces llego cada llamada.
CREATE TABLE intento (
  id          uuid PRIMARY KEY,
  metodo      text NOT NULL,
  payment_id  uuid NOT NULL,
  en          timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX intento_por_pago ON intento (metodo, payment_id);
