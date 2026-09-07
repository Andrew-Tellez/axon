-- The event stream of the `checkout` aggregate. Append-only: `axon verify`
-- blocks any migration that updates or deletes from here, and there is no
-- `.contract.sql` that enables it.
CREATE TABLE checkout_event (
  id         uuid PRIMARY KEY,
  stream_id  uuid NOT NULL,
  version    int  NOT NULL,
  type       text NOT NULL,
  data       jsonb NOT NULL,
  at         timestamptz NOT NULL DEFAULT now(),
  -- This IS the optimistic concurrency. Without it, two writes to the same
  -- stream both get in with the same version, nobody sees an error, and the
  -- state that gets rebuilt depends on what order the rows are read in.
  UNIQUE (stream_id, version)
);

-- The read model. It can be thrown away and rebuilt from the stream: that is
-- what makes it a projection and not a second source of truth.
CREATE TABLE view_conversion (
  stream_id   uuid PRIMARY KEY,
  state       text NOT NULL,
  cents       bigint,
  payment_id  uuid,
  reason      text,
  -- when the event that left the row like this happened: the lag comes from here
  event_at    timestamptz NOT NULL
);

-- How far it got. Without this, a restart reprocesses from the beginning or
-- skips what it did not get to apply; both give a wrong view and neither raises
-- an error.
--
-- And it is PER STREAM. An event's version is its position inside ITS stream, so
-- a single number for the whole view identifies nothing as soon as there is more
-- than one stream: with one it seemed to work, and the failure showed up when
-- rebuilding over several.
CREATE TABLE view_conversion_checkpoint (
  view_name  text NOT NULL,
  stream_id  uuid NOT NULL,
  position   bigint NOT NULL,
  PRIMARY KEY (view_name, stream_id)
);
