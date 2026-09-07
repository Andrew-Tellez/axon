CREATE TABLE payment (
  id            uuid PRIMARY KEY,
  -- no FK: `order` belongs to another service; the id is stored, nothing else
  order_id      uuid NOT NULL,
  amount_cents  bigint NOT NULL,
  currency      text NOT NULL,
  status        text NOT NULL
);

CREATE TABLE payment_attempt (
  id          uuid PRIMARY KEY,
  payment_id  uuid NOT NULL REFERENCES payment(id),
  provider    text NOT NULL,
  failed_at   timestamptz
);
