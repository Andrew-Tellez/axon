# Plugins

A plugin is **any executable on the `PATH` called `axon-*`**. It takes JSON on stdin and
writes on stdout. No ABI, no library loading, no versions to match: `git`'s and
`protoc`'s model. It can be a Go binary or three lines of shell.

| Kind | Invoked by | Receives | Returns |
| --- | --- | --- | --- |
| `axon-gen-<lang>` | `axon build --lang go` | `{manifest, peers}` | source code |
| `axon-infra-<target>` | `axon infra --target pulumi` | the neutral plan | IaC |
| `axon-check-<rule>` | `axon verify` (all of them, always) | every manifest | `[{level, message}]` |

A governance rule of your own, complete:

```sh
#!/bin/sh
# axon-check-names — no service is called "service" or "api"
jq -c '[.[] | select(.service|test("^(service|api)$"))
        | {level:"error", message:("\(.service): generic name not allowed")}]'
```

```console
$ chmod +x axon-check-names && mv axon-check-names ~/.local/bin/
$ axon verify manifests/
error: [axon-check-names] api: generic name not allowed
```

It blocks the pipeline exactly like a native rule.

## The plan a plugin receives, published

`axon-infra-<target>` gets the neutral plan on stdin. Until now its shape had to be
deduced from an example, which means every plugin out there was guessing — and the day a
field appeared, nobody's plugin knew.

```sh
axon infra --schema > plan.schema.json     # JSON Schema 2020-12
```

It mentions no provider, because the plan does not either: rendering it for one is
exactly what a plugin is for. And **every object refuses what it does not declare**,
which is what makes it usable as a contract instead of documentation:

```console
$ axon infra manifests/ --target plan | check-jsonschema --schemafile plan.schema.json -
```

The schema is hand-written, so the only thing keeping it honest is that the suite
validates a **real** plan against it with a real validator, and that the refusal of extra
properties catches drift in both directions: a field added to the plan and not to the
schema fails, and one declared here and missing there fails too.

```
FALLO en workloads/0: Additional properties are not allowed ('invented' was unexpected)
FALLO en workloads/0: 'kind' is a required property
```

## Where the reference generator went

`axon-gen-go` used to live here, as a complete generator written in Go that imported
nothing from axon. Being a plugin was the point for a while: it proved the protocol holds
a real generator and not just a three-line check.

What it could not prove is the claim underneath the whole project — that the manifest is
not TypeScript in disguise — because nobody runs a generator they have to build first. So
Go became **native**: `axon build --lang go`, no `PATH`, no `go build`. Two
implementations of the same generator would have drifted, so there is only one.

The protocol did not change, and neither did the door: what that plugin read on stdin is
what a plugin reads today, and the shape below is still the whole contract.

A generator receives `{manifest, peers}` because a consumed event's schema is declared by
its **emitter**: without the other manifests, no generator can type what its service
receives.
