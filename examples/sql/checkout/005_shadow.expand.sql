-- The view's shadow: the same shape, built aside and swapped for the live one all
-- at once. Rebuilding in place leaves the view incomplete while it runs, and it
-- keeps being read: whoever asks gets fewer rows than there are, with no error at
-- all.
CREATE TABLE view_conversion_shadow (
  stream_id   uuid PRIMARY KEY,
  state       text NOT NULL,
  cents       bigint,
  payment_id  uuid,
  reason      text,
  event_at    timestamptz NOT NULL
);
