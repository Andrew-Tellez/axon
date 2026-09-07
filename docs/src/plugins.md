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

## `axon-gen-go`, the reference generator

[`plugins/axon-gen-go`](https://github.com/Andrew-Tellez/axon/tree/main/plugins/axon-gen-go)
is a complete generator written **in Go** — it imports nothing from axon, its only
contract is the JSON on stdin. It works as a template for any other language:

```sh
go build -o ~/.local/bin/axon-gen-go ./plugins/axon-gen-go
axon build manifests/payments.toml manifests/ --lang go > payments/axon.go
```

It produces idiomatic Go, not translated TypeScript: a handler interface instead of
inheritance, `ctx` first and `error` last, `OrderID` and not `OrderId`, and the output
goes through `go/format` before coming out — a generator should not leave code somebody
has to format afterwards. The suite checks that what it generates passes `go vet`.

It receives `{manifest, peers}` because a consumed event's schema is declared by its
emitter: without the other manifests, no generator can type what its service receives.
