-- El diario de la saga. `axon verify` exige estas columnas y los tipos de las
-- dos ultimas: sin `datos` no se puede retomar —las acciones necesitan la
-- llamada, y el proceso que la tenia en memoria es el que se murio— y con
-- `actualizado` guardado como texto la comparacion del barrido ordena mal.
CREATE TABLE saga_compra (
  id           uuid        PRIMARY KEY,
  paso         int         NOT NULL,
  estado       text        NOT NULL,
  datos        jsonb       NOT NULL,
  -- lo que devolvio cada paso, por numero de paso. Sin esto, compensar despues
  -- de un reinicio es imposible: deshacer el paso 1 necesita el id que ese paso
  -- devolvio, y una variable en memoria no sobrevive al proceso que la tenia.
  salidas      jsonb,
  actualizado  timestamptz NOT NULL DEFAULT now()
);

-- El barrido busca por estado y por fecha, en ese orden.
CREATE INDEX saga_compra_colgadas ON saga_compra (estado, actualizado);
