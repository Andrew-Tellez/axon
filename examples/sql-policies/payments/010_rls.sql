-- generado por axon — no editar:
--   axon rls manifests/ > sql-policies/<servicio>/010_rls.sql
--
-- Va en `sql-policies/`, NO en `sql/`, y se aplica despues de las
-- migraciones. Dos razones: una politica no es un cambio de esquema, y
-- `axon verify` lee `sql/` con un parser SQL que no entiende `DO $$`.
-- El target local ya trae el job que lo aplica a cada nodo.
--
-- COMO SE FIJA EL INQUILINO, y por que importa tanto como la politica:
--
--   BEGIN;
--   SET LOCAL axon.tenant = '<uuid>';   -- LOCAL, no SET a secas
--   ... las consultas ...
--   COMMIT;
--
-- Medido contra Postgres 16, no inferido:
--   * conexion limpia + solo SET LOCAL  -> tras el COMMIT la GUC queda SIN FIJAR
--   * un solo `SET` de sesion, una vez  -> tras el COMMIT, SET LOCAL revierte a ESE
--     valor, no a nada. La fuga persiste aunque el resto del codigo use SET LOCAL
--     correctamente, hasta que alguien haga RESET ALL o se recicle la conexion.
--
-- De ahi la regla, que es mas fuerte que "usá SET LOCAL": NUNCA un `SET` de
-- sesion sobre `axon.tenant`, en ningun lado. Uno solo envenena la conexion para
-- todos los que vengan despues, y si hay un pooler en modo transaccion delante,
-- la siguiente peticion —de OTRO inquilino— recibe esa conexion con el valor
-- anterior puesto. Eso no da error: devuelve las filas del inquilino equivocado.
--
-- Y el rol importa tanto como la politica: un SUPERUSER o un rol con
-- BYPASSRLS se salta TODA politica, y FORCE ROW LEVEL SECURITY no lo remedia.
-- Por eso esta migracion crea `axon_app` y le da a el los permisos. Medido:
-- consultando como el dueno superusuario, la politica no filtra NADA y el
-- resultado es identico al de una base sin RLS. El `SET LOCAL ROLE axon_app`
-- es lo que la enciende, y va junto al del inquilino en la misma transaccion.
--
-- Lo que esta migracion NO puede garantizar: `set_config()` con parametro
-- bindeado —lo que emiten varios ORM— puede no ser interceptado por un pooler;
-- preferi `SET LOCAL` literal.

-- El rol con el que la aplicacion consulta. Sin LOGIN a proposito: se
-- entra con el rol de despliegue y se adopta este dentro de la
-- transaccion, junto con el inquilino, y los dos mueren en el COMMIT:
--
--   BEGIN;
--   SET LOCAL ROLE axon_app;            -- deja de ser superusuario
--   SET LOCAL axon.tenant = '<uuid>';   -- y pasa a ser un inquilino
--   ... las consultas ...
--   COMMIT;
--
-- Sin esto la politica existe y no aplica: el rol por defecto de un
-- Postgres recien creado es superusuario, y la aplicacion "funciona"
-- en local viendo todas las filas de todos los inquilinos.
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'axon_app') THEN
    CREATE ROLE axon_app NOLOGIN NOSUPERUSER NOBYPASSRLS;
  END IF;
END $$;

-- payments.payment: aislamiento por inquilino
ALTER TABLE "payment" ENABLE ROW LEVEL SECURITY;
-- FORCE: la politica aplica tambien al dueno de la tabla, que es
-- quien suele saltarsela sin darse cuenta.
ALTER TABLE "payment" FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS "payment_inquilino" ON "payment";
CREATE POLICY "payment_inquilino" ON "payment"
  USING ("tenant_id" = NULLIF(current_setting('axon.tenant', true), '')::uuid)
  WITH CHECK ("tenant_id" = NULLIF(current_setting('axon.tenant', true), '')::uuid);
-- Los permisos van al rol de la aplicacion, no al dueno: el dueno
-- tiene FORCE encima, pero un superusuario se salta la politica y
-- ninguna clausula lo remedia. Con esto la politica tiene a quien
-- aplicarsele.
GRANT SELECT, INSERT, UPDATE, DELETE ON "payment" TO axon_app;

-- payments.payment_attempt: aislamiento por inquilino
ALTER TABLE "payment_attempt" ENABLE ROW LEVEL SECURITY;
-- FORCE: la politica aplica tambien al dueno de la tabla, que es
-- quien suele saltarsela sin darse cuenta.
ALTER TABLE "payment_attempt" FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS "payment_attempt_inquilino" ON "payment_attempt";
CREATE POLICY "payment_attempt_inquilino" ON "payment_attempt"
  USING ("tenant_id" = NULLIF(current_setting('axon.tenant', true), '')::uuid)
  WITH CHECK ("tenant_id" = NULLIF(current_setting('axon.tenant', true), '')::uuid);
-- Los permisos van al rol de la aplicacion, no al dueno: el dueno
-- tiene FORCE encima, pero un superusuario se salta la politica y
-- ninguna clausula lo remedia. Con esto la politica tiene a quien
-- aplicarsele.
GRANT SELECT, INSERT, UPDATE, DELETE ON "payment_attempt" TO axon_app;
