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
