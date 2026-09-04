# Gobernanza

`axon.policy.toml`, versionado junto al código:

```toml
require_owner          = true
require_tier           = true
allowed_event_prefixes = ["order", "payment", "billing"]
max_deps_per_service   = 7    # si lo pasás, es un monolito distribuido

[ci]                          # el layout del repo es del equipo, no de axon
service_dir = "services/{service}"
test_cmd    = "make -C services/{service} test"
```

Lo que `axon verify` bloquea hoy:

| Chequeo | |
| --- | --- |
| Se consume un evento que nadie emite | error |
| Dos emisores del mismo evento con esquemas distintos | error |
| Se llama un método que el otro servicio no expone | error |
| Dependencia sin `timeout_ms` | error |
| Reintentos sobre un método no idempotente | error |
| Ruta HTTP sin versión, o duplicada entre servicios | error |
| Método mutante sin `idempotent` | error |
| FK que cruza el límite de un servicio | error |
| Migración destructiva sin `.contract.sql` | error |
| Servicio sin `owner` o sin `tier` | error |
| Evento sin consumidores · reintentos sin breaker · demasiadas deps síncronas | aviso |
