# Instalar

Un binario. Sin runtime, sin Node, sin Python, sin JVM. Corre igual en tu laptop y en
un contenedor de CI vacío.

```sh
curl -fsSL https://raw.githubusercontent.com/Andrew-Tellez/axon/main/install.sh | sh
```

macOS y Linux, `arm64` y `x86_64`. El instalador acepta dos variables:

| | |
| --- | --- |
| `AXON_INSTALL_DIR` | Dónde poner el binario. Por defecto `~/.local/bin` |
| `AXON_VERSION` | Una versión concreta, por ejemplo `v0.1.0`. Por defecto la última |

Desde el código, si preferís compilarlo:

```sh
cargo install --git https://github.com/Andrew-Tellez/axon
```

## Para el flujo completo

`axon` solo no necesita nada. Estas herramientas hacen falta para lo que **genera**, y
cada una solo para su parte:

| | Para qué |
| --- | --- |
| Docker | `axon infra --target local` y el sistema en tu máquina |
| Terraform | aplicar lo que sale de `--target gcp` o `--target aws` |
| `kubectl` | aplicar lo que sale de `--target k8s` |
| Node 24+ | correr el testkit de `axon test`, sin paso de build |
| k6 | las pruebas de carga de `axon load` |
| Flyway o similar | aplicar las migraciones; axon las lee, no las ejecuta |
| pgdog | `axon pooler`, si declarás un pooler o reparto |

Ninguna es obligatoria: si falta, lo que se salta es esa parte, no el resto. La lista
completa, y **cómo se verifica lo que genera cada una**, está en el
[README](https://github.com/Andrew-Tellez/axon#con-qué-está-hecho-y-con-qué-se-verifica).
