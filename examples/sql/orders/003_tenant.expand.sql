-- expand: the tenant column, nullable first so nothing live breaks.
ALTER TABLE "order" ADD COLUMN tenant_id uuid;
ALTER TABLE order_item ADD COLUMN tenant_id uuid;
ALTER TABLE "order" ADD COLUMN customer_email text;
