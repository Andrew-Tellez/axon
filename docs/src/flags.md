# Feature flags

What declaring the flags adds is **not the SDK** — [OpenFeature](https://openfeature.dev)
and [flagd](https://flagd.dev) already exist and are better at that. What it adds is that
the compiler enforces what nobody enforces.

```toml
[flags.charge_v2]
owner      = "payments-team"  # whoever turned it on is who turns it off
expires    = "2026-12-31"     # a flag with no death date does not die
rollout    = 10               # per cent
sticky_by  = "tenant_id"      # mandatory with a partial rollout

[flags.stripe_kill]
owner       = "payments-team"
kill_switch = true            # it lives as long as what it switches off
```

## A flag with no death date does not die

Code with two hundred stale flags does not have two hundred features: it has **two
hundred branches nobody tests**. So `expires` is mandatory, and past that date `verify`
fails:

```console
$ axon verify manifests/
error  payments.charge_v2: expired on 2026-12-31. Either the dead branch gets cleaned
       up or the date gets renewed as an explicit decision: leaving it expired is
       neither
```

If it really is permanent —an emergency switch, a per-region cut-off— you declare
`kill_switch = true` and it is exempt. That is the difference between a temporary flag
and an operational control, and it is worth having in writing.

## The rollout has to be sticky

A percentage with no `sticky_by` is evaluated **per request**: the same entity takes one
path on one call and the other on the next, and with state involved it ends up half
migrated.

```console
error  payments.charge_v2: rollout at 10% with no `sticky_by`. Evaluated per request,
       the same entity takes one path and then the other, and ends up half-migrated
```

And the field it is pinned by has to **exist in some contract** —a method input, a field
of an event it emits or consumes, or the tenant column—; otherwise the decision is pinned
by data the service never receives.

The generated accessor requires it in the signature, so it cannot be evaluated per
request even if somebody wanted to:

```ts
export const flagChargeV2 = (flags: Flags, tenant_id: string): Promise<boolean> =>
  flags.evaluate("charge_v2", false, { targetingKey: tenant_id, tenant_id });
```

A `kill_switch` with a `rollout` is an error too: it goes off whole or it is worth
nothing.

## OpenFeature's four types

A flag is not just a boolean. OpenFeature resolves `boolean`, `string`, `number` and
`object`, and a *configuration* rollout —a limit, a provider, a threshold— needs exactly
that:

```toml
[flags.charge_provider]
owner           = "payments-team"
expires         = "2027-06-30"
sticky_by       = "tenant_id"
rollout         = 20
default_variant = "stripe"
variants        = { stripe = "stripe", adyen = "adyen" }

[flags.retry_limit]
owner           = "payments-team"
kill_switch     = true
default_variant = "normal"
variants        = { normal = 3, degraded = 0 }
```

Without `variants`, the flag is the boolean case and the variants are `on` and `off` —
which is what most need and is not worth writing.

The accessor comes out with the right type:

```ts
export const flagChargeProvider = (flags: Flags, tenant_id: string): Promise<string> =>
  flags.evaluate("charge_provider", "stripe", { targetingKey: tenant_id, tenant_id });

export const flagRetryLimit = (flags: Flags): Promise<number> =>
  flags.evaluate("retry_limit", 3, {});
```

And `verify` blocks two errors specific to variants: a `default_variant` that does not
exist in `variants` —evaluation would always fall back to the code's value, and the flag
would silently stop working— and variants that **mix types**, because OpenFeature
resolves one type per flag, not one per variant.

## The configuration, generated

```sh
axon flags manifests/ > flags.json
```

Out comes flagd's configuration, with the rollout expressed in its `fractional` and
pinned by the declared field:

```json
{
  "charge_provider": {
    "state": "ENABLED",
    "variants": { "stripe": "stripe", "adyen": "adyen" },
    "defaultVariant": "stripe",
    "targeting": {
      "fractional": [{ "var": "tenant_id" }, ["adyen", 20], ["stripe", 80]]
    }
  }
}
```

The `local` target brings flagd up with that configuration and passes `AXON_FLAGS_URL` to
each service, so the flag exists on your machine just as it does in production.

## Why OFREP and not flagd's own provider

The [example](https://github.com/Andrew-Tellez/axon/tree/main/examples/services/flags.ts)
uses the OpenFeature SDK with the **OFREP** provider, which is the project's standard
REST protocol: it talks to flagd today and to any other backend that implements it,
without changing a line.

flagd's gRPC provider asks for the old evaluation-service path, and flagd v0.12 already
serves only the new one. The test found that out, not the documentation — and it is the
reason preferring the standard protocol over the specific client is not a matter of
taste.

## Verified, not assumed

`demo.sh` checks the two properties that matter, against a running flagd:

```console
==> declared vs applied rollout
  declared 10%  measured 10.7%  (32 of 300)
  OK: sticky per tenant, and the percentage applies
```

The percentage applies, and **the same entity always gets the same answer**. Without the
second, a payment would take the new path on one call and the old one on the next.
