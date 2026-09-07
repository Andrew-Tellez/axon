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
