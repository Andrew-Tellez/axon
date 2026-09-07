# Architecture: high and low level design

> **The manifest is high-level design** —service boundaries, topology, what each one
> guarantees— **and the compiler lowers it into low-level design**: isolation level, retry
> policy, method signatures, tables, infrastructure resources. And it verifies that they
> stay in agreement.

That sentence is the whole project. This page is what it looks like inside.

## The compiler, end to end

```mermaid
flowchart LR
  subgraph HLD["high-level design · what you declare"]
    M["manifest.toml<br/>one per service"]
    P["axon.policy.toml<br/>the team's rules"]
    B["axon.baseline.json<br/>what is published"]
    SQL["sql/*.sql<br/>the migrations"]
    A["asyncapi.yaml"]
  end

  A -.->|axon import| M
  M --> MODEL
  P --> MODEL
  B --> MODEL
  SQL -->|sqlparser| MODEL

  MODEL["the model<br/>manifest.rs<br/>+ folded schema"]

  MODEL --> V{"axon verify<br/>over 100 rules"}
  V -->|"errors → exit 1"| CI["CI fails<br/>before the merge"]

  MODEL --> LLD

  subgraph LLD["low-level design · what gets derived"]
    CODE["contracts, base class,<br/>resilient clients, testkit"]
    INFRA["the neutral plan<br/>→ IaC for 4 targets"]
    DATA["RLS, masked views,<br/>pgdog, warehouse, metrics"]
    DIAG["mermaid diagrams,<br/>OpenAPI, CI pipeline"]
  end
```

Two properties hold the whole thing up. **Nothing in the low-level column is edited by
hand** — if a diagram disagrees with the code, somebody broke the manifest. And **the
schema is read, never declared**: putting columns in the manifest would be the dual-write
problem dressed up as documentation, so the migrations are the source of truth and
`sqlparser` folds them in order.

## The modules

Each one takes the model and answers a different question. None of them knows about any
cloud provider except the four renderers inside `infra.rs`.

```mermaid
flowchart TB
  MAN["manifest.rs<br/><i>the model, the folded schema,<br/>name helpers</i>"]

  MAN --> VER["verify.rs<br/><i>the rules</i>"]
  MAN --> EMIT["emit.rs<br/><i>TypeScript, CI,<br/>diagrams</i>"]
  MAN --> INF["infra.rs<br/><i>the neutral plan<br/>+ 4 renderers</i>"]
  MAN --> BI["bi.rs<br/><i>warehouse: 3 dialects,<br/>funnels, metrics, drift</i>"]
  MAN --> DBS["dbsec.rs<br/><i>RLS, masked views,<br/>pg_anon</i>"]
  MAN --> POOL["pooler.rs<br/><i>pgdog.toml + users</i>"]
  MAN --> CAP["cap.rs<br/><i>CAP consequences</i>"]
  MAN --> LOAD["carga.rs<br/><i>k6 script + verdict</i>"]
  MAN --> API["api.rs<br/><i>OpenAPI + testkit</i>"]
  MAN --> BASE["baseline.rs<br/><i>published contracts</i>"]
  MAN --> IMP["import.rs<br/><i>AsyncAPI 2.x/3.x</i>"]

  BASE --> VER
  PLUG["plugin.rs<br/><i>axon-gen-* · axon-infra-*<br/>axon-check-*</i>"] --> VER
  EMIT -.-> PLUG
  INF -.-> PLUG

  MAIN["main.rs<br/><i>the CLI: 21 commands</i>"] --> MAN
  COL["color.rs · trace.rs"] --> MAIN
```

Six dependencies in the binary, none accidental: `clap`, `serde` + `toml` +
`serde_json` + `serde_yaml_ng`, `indexmap` (insertion order, so the generated output does
not change between runs and `git diff --exit-code` keeps meaning something), `sqlparser`
and `ureq`.

## Infrastructure, high level: one neutral plan

The manifest mentions **no provider**. `axon infra` builds a plan that mentions none
either, and only then renders it — which is why local and production cannot diverge: they
are two renders of the same plan.

```mermaid
flowchart LR
  subgraph DECL["what the manifest declares"]
    E["[emits.*]"]
    C["[consumes.*]"]
    ME["[methods.*] with http"]
    ST["[infra] state, pool, ha,<br/>backups, replicas, shard_key"]
    BU["[infra.buckets.*]"]
    SE["[infra] secrets"]
    SA["[saga.*] · [aggregate.*]"]
    AN["[analytics]"]
    FL["[flags.*]"]
  end

  E --> PLAN
  C --> PLAN
  ME --> PLAN
  ST --> PLAN
  BU --> PLAN
  SE --> PLAN
  SA --> PLAN
  AN --> PLAN
  FL --> PLAN

  PLAN["<b>the neutral Plan</b><br/>topics · subs · stores · routes<br/>workloads · buckets · secrets<br/>crons · warehouse · flags"]

  PLAN --> L["local<br/><i>docker compose</i>"]
  PLAN --> G["gcp<br/><i>terraform</i>"]
  PLAN --> W["aws<br/><i>terraform</i>"]
  PLAN --> K["k8s<br/><i>Knative Eventing</i>"]
  PLAN --> J["plan<br/><i>JSON, for anything else</i>"]
  J -.-> PG["axon-infra-&lt;target&gt;<br/><i>your own renderer</i>"]
```

The plan is the escape hatch that keeps the model honest: anything a renderer needs has to
exist in the plan first, in neutral vocabulary. A `{project}` placeholder is substituted by
each target with its own syntax, so no provider's grammar leaks into the middle.

## Infrastructure, low level: one declaration, four renders

This is the lowering. One `[emits."order.placed@v1"]` plus one `[consumes]` on the other
side becomes:

```mermaid
flowchart TB
  EV["[emits.&quot;order.placed@v1&quot;]<br/>+ [consumes] in payments"] --> PL["Plan: 1 topic,<br/>1 sub with DLQ"]

  PL --> GG["<b>gcp</b><br/>google_pubsub_topic<br/>+ _topic (dlq)<br/>google_pubsub_subscription<br/>push + OIDC + dead_letter_policy"]
  PL --> AA["<b>aws</b><br/>aws_sns_topic<br/>aws_sqs_queue + redrive<br/>aws_sqs_queue (dlq)<br/>aws_sns_topic_subscription"]
  PL --> KK["<b>k8s</b><br/>Knative Broker<br/>+ Trigger per consumer"]
  PL --> LL["<b>local</b><br/>a JetStream stream<br/>+ a queue subscription"]
```

And the whole mapping, measured on this repo's own example — the counts are what
`axon infra examples --target …` emits today:

| declared | gcp | aws | k8s | local |
| --- | --- | --- | --- | --- |
| an event + its consumer | `google_pubsub_topic` ×2 (+dlq), `google_pubsub_subscription` | `aws_sns_topic`, `aws_sqs_queue` ×2, `aws_sns_topic_subscription` | `Broker` + `Trigger` | JetStream stream + queue sub |
| `[infra] state = "postgres"` | `google_sql_database_instance` + `_database` + `_user` | `aws_db_instance` + `aws_db_parameter_group` | — (no managed database) | one `postgres:16` per node |
| `ha`, `pitr`, `backup_retention_days` | `REGIONAL` + backup config + PITR | `multi_az`, retention (PITR follows) | — | — |
| `read_replicas = 2` | replica instances | `replicate_source_db` | — | — |
| `migrations = "sql/…"` | applied by your pipeline | idem | idem | a Flyway job per node |
| `pii` + `tenant_column` | `axon rls` → `R__rls.sql` | idem | idem | a second Flyway job, own history |
| `[pooler] shards = 4` | **refused** (see below) | **refused** | **refused** | 4 nodes + pgdog with its config |
| a `[methods.*]` with `http` | url_map + backend + NEG | `apigatewayv2_route` + `_integration` | `HTTPRoute` + `Gateway` | Traefik labels |
| the service itself | `google_cloud_run_v2_service` + service account | `aws_ecs_service` + `_task_definition` | `Deployment` + `Service` + `HPA` + `NetworkPolicy` | a container with a healthcheck |
| `[infra.buckets.*]` | `google_storage_bucket` (+ CDN backend) | `aws_s3_bucket` (+ CloudFront) | — | MinIO + a creation job |
| `secrets = […]` | `google_secret_manager_secret` | `aws_secretsmanager_secret` | `ExternalSecret` | `.env.local` |
| `[saga.*]` and snapshots | `google_cloud_scheduler_job` ×2 | `aws_scheduler_schedule` ×2 + a one-shot task | `CronJob` ×2 | a `curl` loop |
| `[analytics] warehouse` | Pub/Sub → BigQuery sub | Firehose → S3 | a Vector `Deployment` | ClickHouse + loader |
| `[flags.*]` | — (your provider) | — | — | flagd + its config |
| every service | OTel env vars, sampling from `tier` | idem | idem | Jaeger + full sampling |

Three of those cells say **refused**, and that is the design:

```mermaid
flowchart LR
  D["[pooler] shards = 4"] --> R{"can the target<br/>render sharding?"}
  R -->|local| Y["4 nodes + pgdog"]
  R -->|"gcp · aws · k8s"| N["<b>error, not one instance</b><br/><i>emitting one would apply with no<br/>error and leave the sharding<br/>non-existent</i>"]
  N --> ESC["--target plan<br/><i>the nodes are in the plan;<br/>render them yourself</i>"]
```

A generator that emits *something* for what it cannot express produces wrong output with
no error, which is the worst failure mode there is. The same refusal covers a warehouse
with no ingest path on the chosen target.

## The verification loop

`verify` is not a linter over one file: it is a comparison between things that live in
different places and drift apart quietly.

```mermaid
flowchart TB
  M1["manifest A"] <-->|"contracts: who emits,<br/>who consumes, what schema"| M2["manifest B"]
  M1 <-->|"a field changed on a<br/>published version"| BL["axon.baseline.json"]
  M1 <-->|"tables, columns, keys,<br/>FKs across boundaries"| SQ["the migrations"]
  M1 <-->|"owner, tier, prefixes,<br/>max deps"| PO["axon.policy.toml"]
  M1 <-->|"your own rules"| PL["axon-check-*"]

  M1 -.->|"axon analytics --check"| WH["the real warehouse"]
  M1 -.->|"axon load --check"| K6["what k6 measured"]
  M1 -.->|"axon discover &lt;url&gt;"| RUN["what is deployed"]
  M1 -.->|"axon seq vs axon trace"| LOG["the envelope log"]
```

The solid lines are what the compiler compares on its own; the dotted ones are where it
compares its declaration against **reality**, and each needs somebody to hand it that
reality — axon does not have, nor want, credentials to your warehouse.

## What the generated code looks like at runtime

The last piece of lowering: the manifest's guarantees become types the compiler can
enforce, and four one-line interfaces you implement.

```mermaid
flowchart TB
  subgraph GEN["generated · axon build"]
    ENV["Envelope&lt;T&gt;<br/><i>traceparent · correlationId · causationId</i>"]
    BASE["abstract &lt;Svc&gt;Service<br/><i>emitters, dispatch(), abstract methods</i>"]
    CLI2["Clients<br/><i>timeout · retries with jitter · breaker</i>"]
    ISO["isolationLevel · maxStalenessMs<br/><i>from the declared CAP side</i>"]
    MACH["&lt;machine&gt;Next() · Can()<br/><i>illegal transition throws</i>"]
    SAGA["run&lt;Saga&gt;() · sweep&lt;Saga&gt;()<br/><i>order, journal, reverse compensation</i>"]
    ES["&lt;agg&gt;Fold() · Load() · Snapshot()<br/>&lt;view&gt;Apply() · rebuild()"]
  end

  subgraph YOURS["yours · ~150 lines in the example"]
    BUS["Bus"]
    INB["Inbox"]
    OUT["Outbox&lt;Tx&gt;<br/><i>tx is mandatory</i>"]
    TR["Transport"]
  end

  BASE --> BUS
  BASE --> INB
  BASE --> OUT
  CLI2 --> TR
  YOURS --> IMPL["your handler bodies"]
```

`Outbox<Tx>`'s mandatory `tx` is the clearest example of the whole idea: the pattern is
not documented, it is made **impossible to get wrong** — the event cannot be written
outside the transaction that changes the state, because there is nowhere to put it.

## How a plugin fits

No ABI, no dynamic loading, no version matching: `git`'s and `protoc`'s model.

```mermaid
sequenceDiagram
  autonumber
  participant U as axon
  participant P as axon-gen-go
  U->>P: spawn (found on the PATH)
  U->>P: stdin · {manifest, peers}
  Note over P: peers, because a consumed<br/>event's schema is owned<br/>by its emitter
  P-->>U: stdout · source code
  P-->>U: exit 0
  Note over U: a non-zero exit, or output<br/>that is not UTF-8, is an error<br/>with the plugin's name in it
```

The same shape covers the three kinds: `axon-gen-<lang>` receives the manifest and its
peers, `axon-infra-<target>` receives the neutral plan, and every `axon-check-*` on the
`PATH` receives every manifest and returns findings that block the pipeline exactly like a
native rule.
