-- El flujo de eventos del agregado `compra`. Append-only: `axon verify`
-- bloquea cualquier migracion que actualice o borre de aqui, y no hay
-- `.contract.sql` que lo habilite.
CREATE TABLE compra_event (
  id         uuid PRIMARY KEY,
  stream_id  uuid NOT NULL,
  version    int  NOT NULL,
  type       text NOT NULL,
  data       jsonb NOT NULL,
  en         timestamptz NOT NULL DEFAULT now(),
  -- Esto ES la concurrencia optimista. Sin el, dos escrituras al mismo flujo
  -- entran las dos con la misma version, nadie ve un error, y el estado que se
  -- reconstruye depende de en que orden se lean las filas.
  UNIQUE (stream_id, version)
);

-- El modelo de lectura. Se puede tirar y reconstruir del flujo: eso es lo que
-- lo hace una proyeccion y no una segunda fuente de verdad.
CREATE TABLE vista_conversion (
  stream_id   uuid PRIMARY KEY,
  estado      text NOT NULL,
  centavos    bigint,
  payment_id  uuid,
  motivo      text,
  -- de cuando es el evento que dejo la fila asi: de aqui sale el atraso
  evento_en   timestamptz NOT NULL
);

-- Hasta donde llego. Sin esto, un reinicio reprocesa desde el principio o se
-- salta lo que no alcanzo a aplicar; las dos cosas dan una vista incorrecta y
-- ninguna da un error.
CREATE TABLE vista_conversion_checkpoint (
  vista     text PRIMARY KEY,
  posicion  bigint NOT NULL
);
