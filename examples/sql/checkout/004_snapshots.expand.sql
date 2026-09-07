-- The aggregate's snapshots. They are a CACHE of the fold: every one of them can
-- be deleted and the system stays correct, only slower. That is what tells them
-- apart from the stream.
CREATE TABLE checkout_snapshot (
  stream_id  uuid NOT NULL,
  version    int  NOT NULL,
  -- Which version of the rules it was computed with. Without this column, an old
  -- snapshot gets rehydrated with new rules and gives a state that no longer
  -- matches replaying the stream, with no error at all.
  rules      int  NOT NULL,
  state      jsonb NOT NULL,
  at         timestamptz NOT NULL DEFAULT now(),
  -- one snapshot per (stream, version, rules): rewriting it is idempotent
  PRIMARY KEY (stream_id, version, rules)
);
