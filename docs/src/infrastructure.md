# Infrastructure

The manifest mentions no provider. `axon infra` first produces a **neutral plan** and
then renders it:

| Target | Edge | Messaging | Compute | State, objects and secrets |
| --- | --- | --- | --- | --- |
| `local` | Traefik | NATS JetStream | your services with `build:` | Postgres + MinIO + Jaeger + migrations applied |
| `gcp` | url_map + backends | Pub/Sub with push and a DLQ | Cloud Run + service account | Cloud SQL, GCS + Cloud CDN, Secret Manager |
| `aws` | API Gateway v2 | SNS → SQS with redrive | ECS Fargate + autoscaling | RDS, S3 + CloudFront, Secrets Manager |
| `k8s` | Gateway API HTTPRoute | Knative Broker/Trigger | Deployment + Service + HPA | External Secrets |
| `plan` | — | — | — | The neutral plan in JSON, to render it yourself |

Every target deploys the complete system: the subscription delivers to a workload that
exists, and the secret reaches the container's environment variable. The image is the
only thing that is not declared — it changes on every deploy, so it comes out as an IaC
variable.

Beyond that, and derived from the same manifest: one topic per event and one
subscription per consumer with a DLQ always, one database per service with its standby,
its backups and its read replicas, the pooler and its shard nodes, the migration jobs
and the RLS policy jobs, the cron that hits the saga sweep and the snapshot prune, the
warehouse ingest path, the OpenTelemetry variables, flagd with its configuration when
there are flags, and —on `local`, when something exports— a Metabase to read the
warehouse with.

## A service that runs and ends

```toml
[infra]
runtime  = "job"          # container · job
schedule = "0 3 * * *"    # five cron fields; absent means somebody triggers it
```

Not everything that runs your business listens on a port. A nightly
recalculation, a backfill, a CLI: declaring them as a container left the only
honest option being to declare nothing, and then their infrastructure lived in
somebody's crontab.

A job is not a container with a different label — every target renders something
else, and each of these was a way of applying with no error and leaving
infrastructure nobody would ever reach:

| | |
| --- | --- |
| `local` | it runs **once** at startup, with `restart: "no"`. There is no scheduler here, and faking one with a `sleep` loop would be inventing an interval the manifest does not have: a cron expression is not a period |
| `k8s` | a `CronJob` with `concurrencyPolicy: Forbid`, or a `Job` with no schedule. Never a Deployment: a pod whose process ends would be restarted forever, looking like a crash loop while doing exactly what it was told |
| `gcp` | a `google_cloud_run_v2_job` plus a scheduler that calls the Run API with **OAuth** — it is Google's API and not the job's, so OIDC would not do. As a service it would be a revision that never becomes ready |
| `aws` | a task definition with no ECS **service**, plus an EventBridge schedule. Its cron is six fields and rejects `*` in both day fields at once, so the day of the week becomes `?` |

And what `verify` refuses, all of it following from "it runs and ends": routes on
a job (they would sit behind an edge that reaches nothing), `min_instances` (an
autoscaler over something that is not up), a `schedule` on something that stays
up, and a schedule that is not five fields — `@daily` is rejected by two of the
three providers. A job that **consumes** events is a warning and not an error:
it is legitimate, and the lag between the event and the reaction is the whole
schedule.

**Local is one more target, not a separate subsystem.** That is why local and production
cannot diverge: they come out of the same declaration.

```sh
axon infra manifests/ --target local > axon.local.yml
docker compose -f axon.local.yml up -d --wait   # broker + postgres + migrations + your services
```

## The demo, in two commands

`examples/` ships three services that really run — `orders`, `payments` and `checkout`,
in TypeScript on Node 24, with no build step. `./demo.sh` brings the whole system up,
fires a flow and checks that reality matches what was declared:

```console
$ cd examples && ./demo.sh
==> POST /v1/tenants/{tenantId}/orders
{"orderId":"64f37016-7ed5-4be5-b45c-bd0074e9df2c"}

==> the real causal chain
flow fce36b59-36c7-4d88-81d1-25d90988e204
└─ POST /v1/tenants/{tenantId}/orders <- http
   └─ order.placed@v1 <- orders
      └─ payment.captured@v1 <- payments

==> expected (manifest) vs real (envelope log)
OK: the system does exactly what it declares
```

That last line is a `diff` between `axon seq --events` and `axon trace --seq`. It runs
in CI on every push.

`axon test` generates the testkit that tests those patterns, and it runs against the
real implementation with `node --test`, with no dependencies:

```console
▶ payments · contract
  ✔ accepts order.placed@v1 exactly as its owner emits it
  ✔ the second delivery of order.placed@v1 does not repeat the effect
  ✔ propagates the causal chain when reacting to order.placed@v1
  ✔ nothing is published outside the outbox
▶ payments · payment machine
  ✔ every declared transition is legal from its source states
  ✔ an undeclared transition blows up
```

The example honours the patterns it declares, it does not simulate them: the payment is
written in the same transaction as its event (a real outbox on Postgres, published by a
relay), redelivering the same envelope does not duplicate the charge (a real inbox), and
the state transition is enforced by `paymentNext()`, which blows up if it is not in the
manifest. The infrastructure adapters are
[150 lines](https://github.com/Andrew-Tellez/axon/blob/main/examples/services/runtime.ts)
— that is everything axon deliberately leaves in the hands of whoever deploys.

Neither the edge nor the storage is a new source of truth. The **API gateway** comes out
of the methods each service already declares with `http`; the **buckets**, out of a block
that decides one important thing:

```toml
[methods.placeOrder]
http       = "POST /v1/orders"
auth       = "public"      # mandatory: the edge fails closed
rate_limit = 60            # mandatory if it is public
timeout_ms = 5000

[infra.buckets.receipts]
retention_days = 2555      # 7 years; without this a bucket grows forever

[infra.buckets.assets]
public    = true           # public ⇒ CDN, and without one it is not public
cache_ttl = 86400
```

`auth` **has no default**: an exposed route with nobody deciding who may call it is an
incident waiting to happen, so `verify` blocks it. And so is a public route with no
`rate_limit` — the edge has nothing to throttle an abuse with.

`public = true` on a bucket is a single decision with two consequences that always go
together: anonymous reads **and** a CDN in front. A private bucket carries no CDN, and a
public one is not left without cache. The bucket's name changes per environment, so it
travels to the container as `BUCKET_<NAME>` — the same variable on all four targets,
pointing at MinIO locally.
