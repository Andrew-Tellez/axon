CREATE TABLE "order" (
  id           uuid PRIMARY KEY,
  customer_id  uuid NOT NULL,
  total_cents  bigint NOT NULL,
  status       text NOT NULL
);

CREATE TABLE order_item (
  id        uuid PRIMARY KEY,
  order_id  uuid NOT NULL REFERENCES "order"(id),
  sku       text NOT NULL,
  qty       int NOT NULL
);
