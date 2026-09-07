-- expand: the attempt log. It exists so they can be COUNTED: the retry policy
-- the manifest declares cannot be checked without knowing how many times each
-- call arrived.
CREATE TABLE attempt (
  id          uuid PRIMARY KEY,
  method      text NOT NULL,
  payment_id  uuid NOT NULL,
  at          timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX attempt_per_payment ON attempt (method, payment_id);
