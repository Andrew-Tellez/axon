CREATE TABLE payment (
  id            uuid PRIMARY KEY,
  order_id      uuid NOT NULL  -- sin FK: `order` es de otro servicio,
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
