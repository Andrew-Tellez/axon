# Rules and drift

What turns the manifest into something more than documentation.

```console
$ axon verify manifests/
error  billing consumes payment.refunded@v1 but nobody emits it
error  payments.payment.stuck is not final and has no way out; it is a deadlock
error  orders.payment.order_id: an FK to payment crosses the service boundary
warn   payments is `strong` and calls orders, which is `eventual`
fail   4 services, 3 errors, 1 warnings
```

It exits with code 1 if there are errors. The errors come first: in a long list, what
has to be fixed cannot end up underneath.

## Contracts

| | |
| --- | --- |
| An event is consumed that nobody emits | error |
| Two emitters of the same event with different schemas | error |
| A method is called that the other service does not expose | error |
| An event is emitted and nobody consumes it | warning |

## Resilience

| | |
| --- | --- |
| A dependency with no `timeout_ms` | error |
| Retries over a method not declared idempotent | error |
| Retries with no `breaker` | warning |
| More synchronous dependencies than the policy's limit | warning |

## API and edge

| | |
| --- | --- |
| An HTTP route with no version, or duplicated across services | error |
| A mutating method with no `idempotent` | error |
| An exposed route with no `auth` | error |
| A public route with no `rate_limit` or no `timeout_ms` | error |
| A paginated method that does not return a `cursor` | error |

## A split manifest

| | |
| --- | --- |
| The same method, event, machine or flag declared in two files | error, naming both |
| The same dependency declared in two files | error |
| A fragment carrying `[infra]`, `[cap]`, `[api]`… | error, saying where it goes |
| An `include` that does not exist, or a directory with no `*.toml` | error |
| An unknown key under `[infra]` | error — a top-level key written after a table belongs to it |

## Declared consumption

| | |
| --- | --- |
| `uses` naming a field the provider does not return | error |
| `uses` naming a field the event does not carry | error |
| A field no consumer reads, with every consumer having declared | warning |
| Removing a published field somebody declared reading | error, naming who |
| Removing a published field nobody declared reading | warning: not a breaking change |

## Versioning and its cycle

| | |
| --- | --- |
| Two services declaring a different `[api]` | error |
| A route with no version under `versioning = "path"` | error |
| A route that versions the path under `versioning = "header"` | error |
| `at` shapes with a `versioning` that is not `"header"` | error |
| An LTS with no `sunset` | error |
| A version served for less than the declared window | error |
| An LTS that dies before a newer version that is not LTS | error |
| Versions out of order, duplicated, or with a malformed date | error |
| A `sunset` already past and still declared | error |
| An `at` shape that changed with no `adapter` | error |
| An `at` shape identical to the current one | error |
| One adapter for two versions | error |
| `sunset` with no `deprecated` on a method | error |
| A method deprecated with no `successor` or no `sunset` | warning |
| Somebody calling a method that is deprecated | warning |
| A `default` that is not the newest version | warning |

## Declared failures

| | |
| --- | --- |
| An `errors` entry with a status that is not 4xx or 5xx | error |
| A code that is not `snake_case` | error |
| The same code declared twice on one method | error |
| `retriable = true` on a 4xx that is not 408, 425 or 429 | error |
| A public mutation with no declared `errors` | warning |
| `retries` against a method whose every declared failure is final | warning |

## Data

| | |
| --- | --- |
| An FK crossing a service boundary | error |
| A table with no tenant column when there is a `tenant_column` | error |
| A table with no shard key when there is a `shard_key` | error |
| An FK between a sharded table and one that is not | error |
| A destructive migration with no `.contract.sql` | error |
| A migration with no numeric prefix | warning |

## State machines

| | |
| --- | --- |
| A state unreachable from the initial one | error |
| A non-final state with no way out — a deadlock | error |
| A transition fired by a method or event that does not exist | error |
| A transition that emits an event the service does not declare | error |
| A compensation pointing at a step that does not exist | error |

## Scaling

| | |
| --- | --- |
| `pool_size` × `max_instances` goes past `max_connections` | error |
| Read replicas under a `strong` promise | error |
| `tier = "0"` with no `ha` or no `backup_retention_days` | error |
| `pitr` with no backups | error |

## Feature flags

| | |
| --- | --- |
| A flag with no `owner` | error |
| A flag with no `expires` that is not a `kill_switch` | error |
| An expired flag | error |
| A partial rollout with no `sticky_by` | error |
| A `kill_switch` with a `rollout` | error |
| `sticky_by` on a field that appears in no contract | error |

## Metrics

| | |
| --- | --- |
| A metric over an event nobody emits | error |
| A `sum` or `avg` over a field that is not a number | error |
| A `sum` or `avg` with no `field` | error |
| A dimension the event does not declare | error |
| A dimension that is a `pii` field | error |
| A `kind` or `window` outside the closed list | error |
| A metric while the service declares `export = false` | error |
| Two services declaring the same metric name | error |
| A `count` with a `field` | warning |

## A published version is immutable

`verify` compares the manifests with each other, but that is not enough for the most
common and most expensive error: **changing a field on a version that is already in
production**, with deployed consumers expecting it as it was. To see that, a record of
what was published is needed.

```console
$ axon baseline manifests/ > manifests/axon.baseline.json   # on publishing
$ axon verify manifests/
error  order.placed@v1.total: changed from `money` to `int` in a published version;
       publish order.placed@v2 instead
```

| | |
| --- | --- |
| A field changes type in a published version | error |
| A field appears or disappears in a published version | error |
| A published event stops being emitted, or changes owner | error |
| An HTTP route moves | error |
| A method stops returning a field, or requires a new one | error |
| New contracts not recorded yet | warning |

In axon **every field is mandatory**, so adding one breaks just as removing one does:
there are no optional fields to make a change "compatible".

If you really want to retire a version, you remove it from `axon.baseline.json` in the
same PR. **That diff is the escape hatch**, and it is reviewed like any other change —
there is no `--force` to be copied without thinking.

Without the file, `verify` warns that it cannot see this class of problem, instead of
staying quiet.

## Your own rules

Every `axon-check-*` on the `PATH` always runs and blocks just like a native rule. See
[Plugins](./plugins.md).
