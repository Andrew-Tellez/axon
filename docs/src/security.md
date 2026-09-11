# Security

Every rule cites its [OWASP Top 10 (2021)](https://owasp.org/Top10/) category, because
an error that does not say *why* it matters gets silenced with an allow:

| | Rule | |
| --- | --- | --- |
| **A01** | A public route that mutates in a `tier = "0"` service | error |
| **A01** | A table with no tenant column when there is a `tenant_column` | error |
| **A02** | A literal secret where its name belongs | error |
| **A04** | A public route with no `timeout_ms` | error |
| **A05** | A public bucket with no `retention_days` | warning |
| **A08** | `[ci].image` with no digest — a tag is mutable | warning |
| **A09** | A field declared `pii` returned by a public route | error |

And what is not warned about is **generated hardened**:

- **A05** — k8s's `Deployment` comes out with `runAsNonRoot`,
  `readOnlyRootFilesystem`, `capabilities: drop ALL`,
  `seccompProfile: RuntimeDefault` and no service account token mounted. Plus a
  deny-by-default `NetworkPolicy`: only the edge gets into the pod.
- **A01** — a service with no public route at all is deployed with
  `ingress = INTERNAL_LOAD_BALANCER`: it has no door to the internet even if somebody
  misconfigures the gateway.
- **A08** — the pipeline builds the image, takes its digest and **deploys by digest**,
  with `provenance: true`.
- **A09** — `axon build` emits `piiFields` and a recursive `redact()`. Personal data
  leaks through a log, not through an exploit.

## Who may call it, and not just that there is somebody

```toml
[api]
scopes = ["orders:read", "payments:write"]   # every scope that exists here

[methods.refundPayment]
http   = "POST /v1/payments/{paymentId}/refunds"
auth   = "required"
scopes = ["payments:write"]
```

`auth = "required"` says the caller is authenticated and **nothing else**: any valid
token, including one issued to read, can issue a refund. A scope is the difference
between *somebody* and *somebody allowed to do this*.

The gateway validates the token — signature, expiry, audience — and hands over what it
granted. That is its job and not the service's. What the service decides is the other
half: whether what was granted covers what this method declared. The generated
`requireScopes` does that, and the list is not retyped in the handler:

```ts
requireScopes("payoutMerchant", granted);   // 403 insufficient_scope, naming the missing one
```

A `403` with no reason is a ticket, so it names what is missing — `insufficient_scope` is
what RFC 6750 calls it. The method is checked against the manifest, so a route cannot
demand a scope nobody declared.

The catalogue in `[api] scopes` is the platform's, and `verify` requires every service to
agree on it: **a scope with a typo is a 403 in production that nobody sees in a review**.
It also refuses a `public` route demanding scopes —nobody presents a token there— and
warns about two things: a mutation behind `required` and nothing else, and a scope in the
catalogue that no method demands, which can be granted to somebody and guards nothing.

The demo measures it against the running service: without the scope, `403` naming it;
with it, `200`; and with a read scope over a payment route, `403` again — which is
exactly the distinction `required` on its own cannot make.

A call between services carries its own credential, not the user's: in the example
`checkout` presents `payments:write` because that is what it needs, and in a real
deployment it comes from its own workload identity. The generated headers propagate the
trace and the idempotency key, never the authorization: that belongs to whoever deploys.

## The shape of the token, and no provider's name

axon authenticates nobody and holds no credential. What `[auth]` declares is the **shape a
verified token must have**, so that the `auth = "required"` at the edge, the `scopes` of a
method and the RLS the compiler already generates stop being three independent hopes that
happen to agree:

```toml
[auth]
issuers   = ["https://auth.acme.mx"]   # a LIST: an IdP migration is weeks accepting two
audience  = "payments"                 # per service
verify    = "jwks"                     # jwks | introspection | adapter
jwks_uri  = "https://auth.acme.mx/.well-known/jwks.json"
algorithms      = ["EdDSA", "ES256"]   # a closed list
revocation      = "eventual"
max_token_age_s = 900

subject_claim = "sub"
tenant_claim  = "org_id"
scopes_claim  = "scope"
roles_claim   = "roles"
```

Who mints the token —better-auth, Auth0, Keycloak, Cognito, thirty lines of `jose`— is an
**adapter**, exactly like `Bus`, `Cache` or `Outbox`. No provider is named in the block,
and that absence is the point: a field that only makes sense for one vendor does not
belong to a compiler. The claim names are the whole provider-specific surface, and they
are data.

What is refused, and why each one has no symptom:

| The rule | What it prevents |
| --- | --- |
| `none` or an `HS*` in `algorithms` | both fail **open**: `none` makes every forged token valid, and an HMAC where a key set is published lets somebody sign with the public key as if it were the secret. Both look like a normal 200 |
| `revocation = "immediate"` with `verify = "jwks"` | verifying a signature offline cannot see a session that was withdrawn: the promise is one the mechanism cannot keep |
| `eventual` with no `max_token_age_s` | the window between withdrawing a token and it stopping is exactly that number, and without it nobody can say how long it is |
| `tenant_column` and no `tenant_claim` | the tenant the RLS binds to would come from the request instead of the token, which is the caller choosing whose rows to read |
| two claim names that are the same | one of the two is reading the wrong thing, and nothing at runtime says which |
| `jwks` with no `jwks_uri` | it typechecks and cannot boot |
| a `jwks_uri` or an `introspection_url` over plaintext `http://` | where the keys come from is as much of the verification as the algorithm: whoever sits on the path serves their own key set and from then on mints tokens the service accepts. The signature checks out, against the wrong keys, and nothing looks broken while it happens. `localhost` is the exception the local target needs |
| two services reading the same claim from different places | it is the same token: one of them gets nothing, and an empty scope list is a 403 that reads as a permissions problem |

And what is **not** refused, on purpose. Three adversarial reviews of this design agreed
on the same trap: a per-service `audience` uniqueness rule breaks Auth0 (one API
identifier for several services is the vendor's own shape) and Cognito (access tokens
carry no `aud` at all), and a single scalar `issuer` breaks every IdP migration, which is
the longest-lived event in an auth system's life. A rule that fires on a correct setup
gets the whole family silenced, and the good ones go with it.

### The verifier, emitted from the block

```sh
axon auth manifests/orders.toml > services/orders/verifier.ts
```

Standard JOSE, and every value that decides whether a token is accepted comes out of the
manifest: the issuers, the key set, the closed list of algorithms, the max age, the claim
names. Changing provider is changing the TOML — the file itself does not name one.

Passing the algorithm list to the library is not a detail: without it, the library trusts
the token's own header about how to verify the token, which is how `none` and the
HMAC-over-a-published-key trick get in.

It refuses where it would have to guess: `introspection` is the issuer's API and its
credential, and `adapter` is you saying you bring your own.

### Roles and plans per endpoint, and the line axon does not cross

```toml
[methods.refundPayment]
auth   = "required"
scopes = ["payments:write"]     # the verb
roles  = ["admin", "support"]   # who
plans  = ["pro", "enterprise"]  # the entitlement that was paid for
```

axon declares **the requirement**, which is contract: it travels in the OpenAPI and in the
generated guard. It does **not** declare the mapping from a role to its scopes — that is
the provider's mutable configuration, which axon can neither observe nor diff, so a second
copy here would go stale in silence and a rule over it would report as an error what
somebody correctly changed on the other side.

The names are checked against `[catalog.role]` and `[catalog.plan]`, so a typo fails in
`verify` instead of being a 403 nobody sees in review. With no catalogue it says so rather
than pretending to check.

### Impersonation

```toml
[auth.impersonation]
claim = "act"          # RFC 8693
roles = ["support"]
audit = true
```

It belongs in the manifest because it decides two things the compiler already reasons
about: **which tenant the RLS binds to** —the impersonated one, or the row is invisible
and the ticket unanswerable— and whether the write says who really did it. `audit = false`
is refused: a change made in somebody else's name with no trace is the worst version of
this, because the row says the customer did it.

The generated `AuthContext` keeps `subject` and `actor` apart, and `withTenant(ctx, tx)`
is the **only** thing that emits `SET LOCAL axon.tenant`. There is no overload taking a
bare string, on purpose: with one, a handler binds the RLS to whatever arrived in the body.

## RLS and masking

```toml
pii = ["customer_email"]

[infra]
tenant_column = "tenant_id"
tenant_exempt = ["audit"]   # what is not business data
```

`axon rls` crosses two things axon already knows — the real schema (read from the
migrations with a SQL parser) and the `pii` fields — and emits one more migration:

```sql
ALTER TABLE "order" ENABLE ROW LEVEL SECURITY;
ALTER TABLE "order" FORCE ROW LEVEL SECURITY;   -- the table's owner too
CREATE POLICY "order_tenant" ON "order"
  USING ("tenant_id" = current_setting('axon.tenant', true)::uuid)
  WITH CHECK ("tenant_id" = current_setting('axon.tenant', true)::uuid);

CREATE OR REPLACE VIEW "order_masked" AS SELECT
  id, customer_id, total_cents, status, tenant_id,
  '[redacted]'::text AS "customer_email"
FROM "order";
REVOKE ALL ON "order" FROM axon_reader;
GRANT SELECT ON "order_masked" TO axon_reader;
```

`verify`'s rule is the one that matters: **a table that forgets the tenant column gets
no policy, and a table with no policy does not fail — it returns everybody's rows.**
That is the silent failure mode the declaration removes.

The suite does not read that SQL: it applies it to a real Postgres and checks that with
no tenant 0 rows are visible, that each tenant sees only its own, that writing for
another one is rejected, and that the view returns `[redacted]`.

It goes in `sql-policies/<service>/R__rls.sql` and not among the migrations: a policy is
not a schema change, and `R__` —Flyway's repeatable prefix— is what makes regenerating
it re-apply it instead of failing with a checksum mismatch.

## A masked copy, with `pg_anon`

The views protect the **live query**: an analytics role never sees the raw data. For the
other problem —giving realistic data to staging, to support, or to a third party that
must not see the real thing— a masked **copy** is needed, and that is what
[`pg_anon`](https://github.com/TantorLabs/pg_anon) does (it is not a Postgres extension
but a CLI that clones the database replacing the fields on the way).

Its biggest friction is maintaining the dictionary of sensitive fields, and its
`create-dict` detects them **by heuristic**. axon emits it declared:

```sh
axon rls manifests/ --target pg_anon > sens_dict.py
pg_anon --mode=dump --prepared-sens-dict-file=sens_dict.py ...
```

The rule comes from each column's **type**, never from its name: guessing because a
column is called `email` fails on `correo` and gets `email_template` wrong. What axon
guarantees is the **coverage** —no field declared `pii` is left without a rule—; the rule
itself is yours and is editable, for example to preserve an address's shape.

The framework's own tables are excluded from the dump: the `outbox` carries every
event's payload in a `jsonb`, which is the last place anyone would look for a leak.

The suite applies every generated rule to a column of its type on a real Postgres —a
missing cast makes the dump fail halfway through— and checks that the original data
survives none of them.
