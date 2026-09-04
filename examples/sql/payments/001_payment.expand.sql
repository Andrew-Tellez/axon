CREATE TABLE payment (
  id            uuid PRIMARY KEY,
  -- sin FK: `order` pertenece a otro servicio; se guarda el id, nada mas
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
