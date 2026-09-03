-- expand: columna nueva, nullable. Compatible con la version anterior del servicio.
ALTER TABLE payment ADD COLUMN provider_ref text;
