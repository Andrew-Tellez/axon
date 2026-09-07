-- The handover to the bus. It is written in the SAME transaction as the stream's
-- event, and the relay publishes from here: the stream is the truth, this table
-- is the delivery. It can be updated and deleted from —it is not the stream— and
-- that is why the relay can mark what it published without violating append-only.
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
