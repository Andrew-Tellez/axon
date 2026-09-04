# Contribuir

## Commits

[Conventional Commits](https://www.conventionalcommits.org/es/), verificados por
[cocogitto](https://github.com/cocogitto/cocogitto). El hook los rechaza antes de
crearlos:

```sh
cargo install cocogitto   # o brew install cocogitto
cog install-hook --all
```

Tipos: los del estándar (`feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`,
`ci`, `chore`, `style`, `revert`) más tres propios de este repo:

| | |
| --- | --- |
| `gen` | Cambios en lo que emite un generador |
| `infra` | Cambios en un target de `axon infra` |
| `sec` | Reglas de seguridad, RLS, enmascarado |

`feat` y `fix` mueven la versión; el resto solo aparece en el changelog. Cambiar la
documentación no es una versión nueva del binario.

El **scope** es la parte que toca: `feat(infra)`, `gen(go)`, `sec(rls)`, `fix(verify)`.

Un `!` o un `BREAKING CHANGE:` en el cuerpo fuerza un salto mayor. **Un cambio
incompatible del formato del manifiesto es breaking**, aunque el código compile: alguien
tiene un `.toml` escrito contra el formato viejo.

### El cuerpo importa más que el título

Este repo tiene una convención propia sobre el cuerpo del mensaje: **explicar por qué,
no qué**. El diff ya dice qué cambió. Lo que se pierde es el razonamiento — y sobre
todo, cuando un cambio corrige un error de diseño, el mensaje dice cuál era el error y
por qué no se veía antes.

## Versionar y liberar

No se escribe el changelog a mano ni se tagea a mano:

```sh
cog bump --auto
```

Lee los commits desde el último tag, decide si el salto es patch, minor o major, escribe
la sección nueva en `CHANGELOG.md`, commitea y tagea. El tag dispara `release.yml`, que
compila los cuatro binarios y publica el release; al terminar, `pages.yml` publica la
documentación de esa versión bajo su propio prefijo.

## Antes de mandar un cambio

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --release              # incluye tsc, go vet, terraform validate y node --test
cargo run --release -- verify examples
cd examples && ./demo.sh          # necesita Docker
mdbook serve docs --open          # la documentacion
```

Encadenalos con `&&`, no con `;`. Con `;` el commit corre igual aunque los tests
fallen — lo hice, y empuje nueve pruebas rojas.

### La regla que rige el suite

**Un generador no se valida con asserts propios: se valida con la herramienta real de
su ecosistema.** El TypeScript pasa por `tsc --strict`, el Go por `go vet`, el Terraform
por `terraform validate` con los providers de verdad, el testkit por `node --test`
contra el servicio de ejemplo, y la RLS se aplica a un Postgres real para comprobar que
aísla.

Esto no es celo: los tres primeros generadores producían salida inválida y el suite no
lo veía, porque axon se verificaba únicamente contra sí mismo.

Si agregás un generador, agregá su verificación externa en el mismo cambio. Si la
herramienta no está instalada, el test se **saltea** — nunca miente diciendo que pasó.

## Qué entra y qué no

axon genera lo que se **declara**; la implementación es de quien escribe el servicio.
La línea no es de gusto:

- **Entra** lo que cruza procesos o es una *política*: contratos, topología, límites,
  timeouts, transiciones de estado, recursos de infraestructura, reglas de acceso.
  Todo eso es declarable y, sobre todo, **verificable**.
- **No entra** el cuerpo de un handler, ni los algoritmos, ni la descomposición interna
  del dominio. Ahí murieron MDA, Rational Rose y el low-code: expresar todo el
  comportamiento en un manifiesto termina siendo un lenguaje de programación nuevo y
  peor que los seis a los que compila.

Un patrón nuevo entra si el compilador puede **imponerlo o refutarlo**. Si lo único que
puede hacer es documentarlo, va en la documentación.
