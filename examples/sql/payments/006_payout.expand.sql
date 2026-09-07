-- expand: the payout to the merchant. It is recorded so it can be idempotent: a
-- retry with the same payment does not pay twice.
CREATE TABLE payout (
  id          uuid PRIMARY KEY,
  payment_id  uuid NOT NULL UNIQUE,
  tenant_id   uuid,
  cents       bigint NOT NULL,
  created_at  timestamptz NOT NULL DEFAULT now()
);
