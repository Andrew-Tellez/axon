-- La vista enmascarada que generaba `axon rls` se llamaba `order_enmascarada`
-- y ahora se llama `order_masked`. El archivo de politicas se regenera entero,
-- asi que el nombre nuevo aparece solo; el viejo NO se va por si mismo: queda
-- una vista huerfana que sigue respondiendo. Aca no expone nada —enmascara la
-- misma columna— pero una vista de mas es una superficie de mas.
DROP VIEW IF EXISTS order_enmascarada;
