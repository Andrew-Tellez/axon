# Feature flags

Lo que aporta declarar los flags **no es el SDK** — [OpenFeature](https://openfeature.dev)
y [flagd](https://flagd.dev) ya existen y son mejores en eso. Lo que aporta es que el
compilador imponga lo que nadie impone.

```toml
[flags.cobro_v2]
owner      = "equipo-pagos"   # el que lo prendió es el que lo apaga
expires    = "2026-12-31"     # un flag sin fecha de muerte no muere
rollout    = 10               # por ciento
sticky_by  = "tenant_id"      # obligatorio con rollout parcial

[flags.cortar_stripe]
owner       = "equipo-pagos"
kill_switch = true            # vive mientras exista lo que apaga
```

## Un flag sin fecha de muerte no muere

Un código con doscientos flags viejos no tiene doscientas features: tiene **doscientas
ramas que nadie prueba**. Así que `expires` es obligatorio, y pasada esa fecha `verify`
falla:

```console
$ axon verify manifests/
error  payments.cobro_v2: vencio el 2026-12-31. O se limpia la rama muerta, o se
       renueva la fecha con una decision explicita: dejarlo vencido no es ninguna
       de las dos
```

Si de verdad es permanente —un interruptor de emergencia, un corte por región— se
declara `kill_switch = true` y queda exento. Esa es la diferencia entre un flag temporal
y un control operativo, y conviene que esté escrita.

## El rollout tiene que ser fijo

Un porcentaje sin `sticky_by` se evalúa **por petición**: la misma entidad toma un camino
en una llamada y el otro en la siguiente, y con estado de por medio queda a medio migrar.

```console
error  payments.cobro_v2: rollout al 10% sin `sticky_by`. Evaluado por peticion, la
       misma entidad toma un camino y despues el otro, y queda a medio migrar
```

Y el campo por el que se fija tiene que **existir en algún contrato** —una entrada de
método, un campo de un evento que emite o consume, o la columna del inquilino—; si no, la
decisión se fija por un dato que el servicio nunca recibe.

El accesor generado lo exige en la firma, así que no se puede evaluar por petición
aunque uno quiera:

```ts
export const flagCobroV2 = (flags: Flags, tenant_id: string): Promise<boolean> =>
  flags.evaluar("cobro_v2", false, { targetingKey: tenant_id, tenant_id });
```

Un `kill_switch` con `rollout` también es un error: se apaga entero o no sirve de nada.

## Los cuatro tipos de OpenFeature

Un flag no es solo un booleano. OpenFeature resuelve `boolean`, `string`, `number` y
`object`, y un rollout de *configuración* —un límite, un proveedor, un umbral— necesita
justamente eso:

```toml
[flags.proveedor_de_cobro]
owner           = "equipo-pagos"
expires         = "2027-06-30"
sticky_by       = "tenant_id"
rollout         = 20
default_variant = "stripe"
variants        = { stripe = "stripe", adyen = "adyen" }

[flags.limite_de_reintentos]
owner           = "equipo-pagos"
kill_switch     = true
default_variant = "normal"
variants        = { normal = 3, degradado = 0 }
```

Sin `variants`, el flag es el caso booleano y las variantes son `on` y `off` — que es lo
que necesita la mayoría y no vale la pena escribir.

El accesor sale con el tipo correcto:

```ts
export const flagProveedorDeCobro = (flags: Flags, tenant_id: string): Promise<string> =>
  flags.evaluar("proveedor_de_cobro", "stripe", { targetingKey: tenant_id, tenant_id });

export const flagLimiteDeReintentos = (flags: Flags): Promise<number> =>
  flags.evaluar("limite_de_reintentos", 3, {});
```

Y `verify` bloquea dos errores propios de las variantes: un `default_variant` que no
existe en `variants` —la evaluación caería siempre al valor del código, y el flag
dejaría de servir en silencio— y variantes que **mezclan tipos**, porque OpenFeature
resuelve un tipo por flag, no uno por variante.

## La configuración, generada

```sh
axon flags manifests/ > flags.json
```

Sale la configuración de flagd, con el rollout expresado en su `fractional` y fijado por
el campo declarado:

```json
{
  "proveedor_de_cobro": {
    "state": "ENABLED",
    "variants": { "stripe": "stripe", "adyen": "adyen" },
    "defaultVariant": "stripe",
    "targeting": {
      "fractional": [{ "var": "tenant_id" }, ["adyen", 20], ["stripe", 80]]
    }
  }
}
```

El target `local` levanta flagd con esa configuración y le pasa `AXON_FLAGS_URL` a cada
servicio, así que el flag existe en tu máquina igual que en producción.

## Por qué OFREP y no el proveedor de flagd

El [ejemplo](https://github.com/Andrew-Tellez/axon/tree/main/examples/services/flags.ts)
usa el SDK de OpenFeature con el proveedor **OFREP**, que es el protocolo REST estándar
del proyecto: habla con flagd hoy y con cualquier otro backend que lo implemente, sin
cambiar una línea.

El proveedor gRPC de flagd pide la ruta vieja del servicio de evaluación, y flagd v0.12
ya sirve solo la nueva. Eso lo encontró la prueba, no la documentación — y es la razón
por la que preferir el protocolo estándar sobre el cliente específico no es una cuestión
de gusto.

## Verificado, no supuesto

El `demo.sh` comprueba las dos propiedades que importan, contra flagd corriendo:

```console
==> rollout declarado vs aplicado
  declarado 10%  medido 10.7%  (32 de 300)
  OK: estable por inquilino, y el porcentaje aplica
```

El porcentaje se aplica, y **la misma entidad recibe siempre la misma respuesta**. Sin lo
segundo, un pago tomaría el camino nuevo en una llamada y el viejo en la siguiente.
