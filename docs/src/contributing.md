# Contributing

The full text is in
[`CONTRIBUTING.md`](https://github.com/Andrew-Tellez/axon/blob/main/CONTRIBUTING.md).
The essentials:

## Commits

[Conventional Commits](https://www.conventionalcommits.org/), verified by
[cocogitto](https://github.com/cocogitto/cocogitto):

```sh
cargo install cocogitto
cog install-hook --all
```

Types of this repo's own, on top of the standard ones: `gen` (what a generator emits),
`infra` (an `axon infra` target), `sec` (security rules, RLS, masking).

**The body matters more than the subject**: explain *why*, not *what*. The diff already
says what changed; what gets lost is the reasoning — and above all, when a change fixes a
design mistake, what the mistake was and why it was not visible before.

`cog bump --auto` writes the changelog and the tag; none of that is done by hand.

## The rule that governs the suite

**A generator is not validated with its own asserts: it is validated with the real tool of
its ecosystem.** The TypeScript goes through `tsc --strict`, the Go through `go vet`, the
Terraform through `terraform validate` with the real providers, the testkit through
`node --test` against the example service, and the RLS is applied to a real Postgres to
check that it isolates.

This is not zeal: the first three generators produced invalid output and the suite did not
see it, because axon was only ever verified against itself.

If you add a generator, add its external verification in the same change. If the tool is
not installed, the test **skips** — it never lies by saying it passed.

## What gets in and what does not

axon generates what is **declared**; the implementation belongs to whoever writes the
service. The line is not a matter of taste:

- **In**: whatever crosses processes or is a *policy*. All of that is declarable and,
  above all, **verifiable**.
- **Out**: a handler's body, the algorithms, the domain's internal decomposition. That is
  where MDA, Rational Rose and low-code died.

A new pattern gets in if the compiler can **enforce it or refute it**. If all it can do is
document it, it belongs in the documentation.

## This documentation

It lives in `docs/src/`, is built with [mdBook](https://rust-lang.github.io/mdBook/) and
is published on every push to `main`:

```sh
cargo install mdbook
mdbook serve docs --open
```

**The manifest examples are not text**: the suite extracts every ` ```toml ` block from
these pages and runs `axon verify` over it. And the output these pages quote is looked for
in the source that prints it: four consecutive words of each quoted message have to appear
in some message of `src/`. An example that does not validate, or a quote that no longer
matches, breaks CI — so the documentation cannot go stale in silence.
