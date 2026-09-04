-- expand: la columna del inquilino, nullable primero para no romper lo vivo.
ALTER TABLE "order" ADD COLUMN tenant_id uuid;
ALTER TABLE order_item ADD COLUMN tenant_id uuid;
ALTER TABLE "order" ADD COLUMN customer_email text;
