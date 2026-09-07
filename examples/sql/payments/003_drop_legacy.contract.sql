-- contract: destructive on purpose, after nobody reads the column any more.
ALTER TABLE payment DROP COLUMN currency;
