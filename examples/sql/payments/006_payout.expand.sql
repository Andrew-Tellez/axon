-- expand: el pago al comercio. Se registra para poder ser idempotente: un
-- reintento con el mismo pago no paga dos veces.
CREATE TABLE payout (
  id          uuid PRIMARY KEY,
  payment_id  uuid NOT NULL UNIQUE,
  tenant_id   uuid,
  cents       bigint NOT NULL,
  created_at  timestamptz NOT NULL DEFAULT now()
);
