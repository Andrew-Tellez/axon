-- El traspaso al bus. Se escribe en la MISMA transaccion que el evento del
-- flujo, y de aqui publica el relay: el flujo es la verdad, esta tabla es la
-- entrega. Se puede actualizar y borrar —no es el flujo— y por eso el relay
-- puede marcar lo publicado sin violar el append-only.
CREATE TABLE outbox (
  id             uuid PRIMARY KEY,
  type           text NOT NULL,
  source         text NOT NULL,
  time           text NOT NULL,
  traceparent    text NOT NULL,
  correlation_id uuid NOT NULL,
  causation_id   uuid,
  data           jsonb NOT NULL,
  published_at   timestamptz
);
