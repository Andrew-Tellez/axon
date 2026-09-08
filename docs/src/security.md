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
