-- Las dos tablas que exigen los patrones declarados en el manifiesto.
CREATE TABLE inbox_seen (
  id           uuid PRIMARY KEY,
  seen_at      timestamptz NOT NULL DEFAULT now()
);

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
