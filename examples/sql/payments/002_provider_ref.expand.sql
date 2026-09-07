-- expand: a new column, nullable. Compatible with the service's previous version.
ALTER TABLE payment ADD COLUMN provider_ref text;
