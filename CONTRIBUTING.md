# Contributing

## Commits

[Conventional Commits](https://www.conventionalcommits.org/), verified by
[cocogitto](https://github.com/cocogitto/cocogitto). The hook rejects them before they
are created:

```sh
cargo install cocogitto   # or brew install cocogitto
cog install-hook --all
```

Types: the standard ones (`feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`,
`ci`, `chore`, `style`, `revert`) plus three of this repo's own:

| | |
| --- | --- |
| `gen` | Changes in what a generator emits |
| `infra` | Changes in an `axon infra` target |
| `sec` | Security rules, RLS, masking |

`feat` and `fix` move the version; everything else only shows up in the changelog.
Changing the documentation is not a new version of the binary.

The **scope** is the part it touches: `feat(infra)`, `gen(go)`, `sec(rls)`,
`fix(verify)`.

A `!` or a `BREAKING CHANGE:` in the body forces a major bump. **An incompatible change
to the manifest format is breaking**, even if the code compiles: somebody out there has
a `.toml` written against the old format.

### The body matters more than the subject

This repo has a convention of its own about the message body: **explain why, not what**.
The diff already says what changed. What gets lost is the reasoning — and above all,
when a change fixes a design mistake, the message says what the mistake was and why it
was not visible before.

## Versioning and releasing

The changelog is not written by hand and the tags are not created by hand:

```sh
cog bump --auto
```

It reads the commits since the last tag, decides whether the bump is patch, minor or
major, writes the new section into `CHANGELOG.md`, commits and tags. The tag triggers
`release.yml`, which builds the four binaries and publishes the release; when that
finishes, `pages.yml` publishes that version's documentation under its own prefix.

## Before sending a change

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --release              # includes tsc, go vet, terraform validate, mermaid and node --test
cargo run --release -- verify examples
cd examples && ./demo.sh          # needs Docker
mdbook serve docs --open          # the documentation
```

Chain them with `&&`, not with `;`. With `;` the commit goes through even if the tests
fail — I did that, and pushed nine red tests.

### The rule that governs the suite

**A generator is not validated with its own asserts: it is validated with the real tool
of its ecosystem.** The TypeScript goes through `tsc --strict`, the Go through `go vet`,
the Terraform through `terraform validate` with the real providers, the testkit through
`node --test` against the example service, the five diagrams through **mermaid itself**,
the OpenAPI through **`redocly lint --extends=spec`** —conformance, not Redocly's
taste— and the RLS is applied to a real Postgres to check that it isolates.

The two that run on Node need their dependencies, like the example's `tsc` does:

```sh
cd tests/js && npm i
```

and the same file checks a diagram by hand, before pasting it anywhere:

```sh
axon classes . | node tests/js/mermaid.mjs
```

This is not zeal: the first three generators produced invalid output and the suite did
not see it, because axon was only ever verified against itself.

If you add a generator, add its external verification in the same change. If the tool is
not installed, the test **skips** — it never lies by saying it passed.

## What gets in and what does not

axon generates what is **declared**; the implementation belongs to whoever writes the
service. The line is not a matter of taste:

- **In**: whatever crosses processes or is a *policy*: contracts, topology, limits,
  timeouts, state transitions, infrastructure resources, access rules. All of that is
  declarable and, above all, **verifiable**.
- **Out**: a handler's body, the algorithms, the domain's internal decomposition. That
  is where MDA, Rational Rose and low-code died: expressing all behaviour in a manifest
  ends up being a new programming language, and a worse one than the six it compiles to.

A new pattern gets in if the compiler can **enforce it or refute it**. If all it can do
is document it, it belongs in the documentation.
