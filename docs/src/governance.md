# Governance

`axon.policy.toml`, versioned alongside the code:

```toml
require_owner          = true
require_tier           = true
allowed_event_prefixes = ["order", "payment", "billing"]
max_deps_per_service   = 7    # past this, it is a distributed monolith

[ci]                          # the repo layout belongs to the team, not to axon
service_dir = "services/{service}"
test_cmd    = "make -C services/{service} test"
```

What `axon verify` blocks today:

| Check | |
| --- | --- |
| An event is consumed that nobody emits | error |
| Two emitters of the same event with different schemas | error |
| A method is called that the other service does not expose | error |
| A dependency with no `timeout_ms` | error |
| Retries over a non-idempotent method | error |
| An HTTP route with no version, or duplicated across services | error |
| A mutating method with no `idempotent` | error |
| An FK crossing a service boundary | error |
| A destructive migration with no `.contract.sql` | error |
| A service with no `owner` or no `tier` | error |
| An event with no consumers · retries with no breaker · too many synchronous deps | warning |
