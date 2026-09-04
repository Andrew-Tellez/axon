# Entrar sin reescribir nada

Si el equipo ya tiene un catálogo de eventos, el manifiesto no se escribe a mano:

```console
$ axon import asyncapi eventos.yaml > manifests/shipping.toml
$ axon verify manifests/
error: shipping-service: sin `owner`; un servicio sin dueno no se despliega
error: shipping-service: sin `tier`; la criticidad decide alertas y SLO
```

Lee AsyncAPI **2.x y 3.x**, en JSON o YAML, y traduce la semántica invertida de 2.x
correctamente (`publish` es lo que la app *recibe*, `subscribe` lo que *emite* — al
revés de lo que sugieren las palabras). Mapea `format: uuid`, `date-time` y detecta
`{amount, currency}` como `money`.

Lo que el documento no dice — dueño, criticidad, timeouts — sale como `TODO`, y
`verify` lo reclama: **un placeholder no es un valor**. El import te deja en un
estado incompleto pero honesto, nunca en uno que finge estar listo.
