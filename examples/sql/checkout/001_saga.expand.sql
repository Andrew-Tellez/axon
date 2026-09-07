-- The saga's journal. `axon verify` requires these columns and the types of the
-- last two: without `data` it cannot be resumed —the actions need the call, and
-- the process that had it in memory is the one that died— and with `updated`
-- stored as text the sweep's comparison sorts wrong.
CREATE TABLE saga_checkout (
  id        uuid        PRIMARY KEY,
  step      int         NOT NULL,
  status    text        NOT NULL,
  data      jsonb       NOT NULL,
  -- what each step returned, by step number. Without this, compensating after a
  -- restart is impossible: undoing step 1 needs the id that step returned, and a
  -- variable in memory does not survive the process that held it.
  outputs   jsonb,
  updated   timestamptz NOT NULL DEFAULT now()
);

-- The sweep looks by status and by date, in that order.
CREATE INDEX saga_checkout_stranded ON saga_checkout (status, updated);
