# Contribuir

El texto completo está en
[`CONTRIBUTING.md`](https://github.com/Andrew-Tellez/axon/blob/main/CONTRIBUTING.md).
Lo esencial:

## Commits

[Conventional Commits](https://www.conventionalcommits.org/es/), verificados por
[cocogitto](https://github.com/cocogitto/cocogitto):

```sh
cargo install cocogitto
cog install-hook --all
```

Tipos propios de este repo, además de los del estándar: `gen` (lo que emite un
generador), `infra` (un target de `axon infra`), `sec` (reglas de seguridad, RLS,
enmascarado).

**El cuerpo importa más que el título**: explicá *por qué*, no *qué*. El diff ya dice
qué cambió; lo que se pierde es el razonamiento — y sobre todo, cuando un cambio corrige
un error de diseño, cuál era el error y por qué no se veía antes.

`cog bump --auto` escribe el changelog y el tag; nada de eso se hace a mano.

## La regla que rige el suite

**Un generador no se valida con asserts propios: se valida con la herramienta real de su
ecosistema.** El TypeScript pasa por `tsc --strict`, el Go por `go vet`, el Terraform por
`terraform validate` con los providers de verdad, el testkit por `node --test` contra el
servicio de ejemplo, y la RLS se aplica a un Postgres real para comprobar que aísla.

Esto no es celo: los tres primeros generadores producían salida inválida y el suite no lo
veía, porque axon se verificaba únicamente contra sí mismo.

Si agregás un generador, agregá su verificación externa en el mismo cambio. Si la
herramienta no está instalada, el test se **saltea** — nunca miente diciendo que pasó.

## Qué entra y qué no

axon genera lo que se **declara**; la implementación es de quien escribe el servicio.
La línea no es de gusto:

- **Entra** lo que cruza procesos o es una *política*. Todo eso es declarable y, sobre
  todo, **verificable**.
- **No entra** el cuerpo de un handler, ni los algoritmos, ni la descomposición interna
  del dominio. Ahí murieron MDA, Rational Rose y el low-code.

Un patrón nuevo entra si el compilador puede **imponerlo o refutarlo**. Si lo único que
puede hacer es documentarlo, va en la documentación.

## Esta documentación

Está en `docs/src/`, se construye con [mdBook](https://rust-lang.github.io/mdBook/) y
se publica en cada push a `main`:

```sh
cargo install mdbook
mdbook serve docs --open
```

**Los ejemplos de manifiesto no son texto**: el suite extrae cada bloque ` ```toml ` de
estas páginas y le corre `axon verify`. Un ejemplo que no valida rompe CI, así que la
documentación no puede quedar vieja en silencio.
