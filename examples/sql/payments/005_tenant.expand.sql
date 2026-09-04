ALTER TABLE payment ADD COLUMN tenant_id uuid;
ALTER TABLE payment_attempt ADD COLUMN tenant_id uuid;
