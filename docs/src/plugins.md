# Plugins

Un plugin es **cualquier ejecutable en el `PATH` llamado `axon-*`**. Recibe JSON por
stdin, escribe por stdout. Sin ABI, sin cargar librerías, sin versiones que casen:
el modelo de `git` y de `protoc`. Puede ser un binario de Go o tres líneas de shell.

| Clase | Se invoca con | Recibe | Devuelve |
| --- | --- | --- | --- |
| `axon-gen-<lang>` | `axon build --lang go` | `{manifest, peers}` | código fuente |
| `axon-infra-<target>` | `axon infra --target pulumi` | el plan neutral | IaC |
| `axon-check-<regla>` | `axon verify` (todos, siempre) | todos los manifiestos | `[{level, message}]` |

Una regla de gobernanza propia, completa:

```sh
#!/bin/sh
# axon-check-nombres — ningún servicio se llama "service" o "api"
jq -c '[.[] | select(.service|test("^(service|api)$"))
        | {level:"error", message:("\(.service): nombre genérico prohibido")}]'
```

```console
$ chmod +x axon-check-nombres && mv axon-check-nombres ~/.local/bin/
$ axon verify manifests/
error: [axon-check-nombres] api: nombre generico prohibido
```

Bloquea el pipeline exactamente igual que una regla nativa.

## `axon-gen-go`, el generador de referencia

[`plugins/axon-gen-go`](plugins/axon-gen-go) es un generador completo escrito **en Go**
— no importa nada de axon, su único contrato es el JSON de stdin. Sirve de plantilla
para cualquier otro lenguaje:

```sh
go build -o ~/.local/bin/axon-gen-go ./plugins/axon-gen-go
axon build manifests/payments.toml manifests/ --lang go > payments/axon.go
```

Produce Go idiomático, no TypeScript traducido: interfaz de handlers en vez de herencia,
`ctx` primero y `error` al final, `OrderID` y no `OrderId`, y la salida pasa por
`go/format` antes de salir — un generador no debería dejar código que alguien tenga que
formatear después. El suite comprueba que lo generado pase `go vet`.

Recibe `{manifest, peers}` porque el esquema de un evento consumido lo declara su
emisor: sin los demás manifiestos, ningún generador puede tipar lo que su servicio
recibe.
