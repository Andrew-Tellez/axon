# Getting in without rewriting anything

If the team already has an event catalogue, the manifest is not written by hand:

```console
$ axon import asyncapi events.yaml > manifests/shipping.toml
$ axon verify manifests/
error: shipping-service: no `owner`; a service with no owner does not get deployed
error: shipping-service: no `tier`; criticality decides alerts and SLO
```

It reads AsyncAPI **2.x and 3.x**, in JSON or YAML, and translates 2.x's inverted
semantics correctly (`publish` is what the app *receives*, `subscribe` what it *emits* —
the reverse of what the words suggest). It maps `format: uuid`, `date-time` and detects
`{amount, currency}` as `money`.

What the document does not say — owner, criticality, timeouts — comes out as `TODO`, and
`verify` demands it: **a placeholder is not a value**. The import leaves you in an
incomplete but honest state, never in one that pretends to be ready.

## From OpenAPI: what an HTTP service already has

An event catalogue is a decision somebody made. An OpenAPI document usually is not: a
NestJS repo has one from its decorators, a FastAPI one from its types. It is there
whether or not anybody planned it, and it already says the routes, the shapes and the
statuses.

```console
$ axon import openapi swagger.json > manifests/billing.toml
$ axon verify manifests/
error billing-service: no `owner`; a service with no owner does not get deployed
error billing-service.create: POST /invoices mutates with no `idempotent = true`
error billing-service.findAll: public route with no `timeout_ms`
error billing-service.findOne: `/invoices/{invoiceId}` has no version in the path
```

What it reads: the routes and their parameters —path and query, because a header is
transport and not contract—, the request and response bodies as `in` and `out`, `uuid`,
`date-time` and `{amount, currency}` as their own types, an array as `json` because a
list is not a scalar, and **the declared statuses as declared failures**, which is the
one thing an OpenAPI has that an AsyncAPI does not. `security` decides `auth`, and an
explicit `security: []` overrides the global one — getting that backwards would mark a
whole private API as public.

Nest writes `InvoicesController_findOne` as the operationId; the controller is already
named by the route, so the method keeps the half that is the method.

**What it refuses to invent is as important as what it reads.** A timeout is not in the
document, and writing `timeout_ms = 3000` would be a number nobody decided that looks
decided. `idempotent = true` is worse: it is a claim about code this importer has never
seen. Both come out commented, and `verify` demands them where they matter, so a person
answers.

## The line: `axon accept`

`verify` is all-or-nothing, and that is what keeps it out of a codebase that already
exists. The first run on a repo with twenty services prints two hundred warnings, nobody
reads them, and the tool is off within a week. It is the same thing that happens to a
linter with no suppressions file.

```console
$ axon verify manifests/          # day one
warn  billing: no `[cap]`; assumed CP (strong/reject), which fails closed
warn  billing.charge: paginated but does not return a `cursor`; offset breaks as it grows
...                               # and another 198

$ axon accept manifests/ > axon.accepted.json
$ axon verify manifests/
      3 warnings accepted in axon.accepted.json
ok 1 services, 0 errors, 0 warnings (3 accepted)
```

From there, **a warning that is not on the list fails the build**:

```console
$ axon verify manifests/
new billing: exports 1 hashed personal fields to the warehouse
      3 warnings accepted in axon.accepted.json
fail 1 services, 0 errors, 1 warnings (3 accepted)
```

Two things keep the file from becoming the drawer warnings go into to be forgotten:

- **Its presence is the opt-in.** There is no flag to remember: the file exists because
  somebody decided to draw the line, and from then on it holds.
- **The list can only shrink without anybody noticing.** An accepted warning that stopped
  happening is reported —`gone  1 accepted warning no longer happens`— so the file tracks
  the repo getting better instead of accumulating.

It stores the exact messages and not rule ids, because there are no rule ids: here the
message *is* the rule. An axon that changes its wording makes the file stale on purpose,
and re-running `axon accept` is a diff somebody reads in the PR — the same deal as
`axon.baseline.json` for published contracts.

Adoption, then, is four steps and no big bang:

1. `axon import` or one manifest written by hand, `axon verify` in CI. Nothing is
   generated yet: it only blocks drift.
2. `axon baseline` on day one, so a breaking change to something already published fails
   in the PR.
3. `axon accept`, so the two hundred warnings of the past do not drown the one from today.
4. One generator at a time — `axon build` for one service's clients — and
   `axon infra --target local` long before touching production.
