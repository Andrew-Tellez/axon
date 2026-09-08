# Pactos de consumidores que no usan axon

`mobile-app` es un equipo ajeno: no tiene manifiesto y nunca va a tenerlo. Lo que
sí tiene, porque usa Pact, es un archivo que dice exactamente qué campos
necesita de `orders`.

```sh
axon pact . --check pacts/mobile-app-orders.json
```

axon no necesita un broker para **leer** un pacto. Cruza lo que el consumidor
espera contra lo que el proveedor declara y contesta las dos preguntas que un
consumidor ajeno deja sin respuesta: si espera un campo que nadie devuelve —lo
renombraron, o el pacto quedó viejo— y **qué campos declarados este consumidor
no lee**, que es la pregunta que descongela un contrato.

No es permiso para borrar: otro consumidor puede leerlo. Es un nombre menos en
la lista de desconocidos.
