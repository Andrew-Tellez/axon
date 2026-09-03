"""Un solo check: si la logica se rompe, esto falla. python3 test_axon.py"""
import axon

BASE = [
    {"_path": "a", "service": "orders", "emits": {"order.placed@v1": {"id": "uuid"}},
     "methods": {"getOrder": {"in": {"id": "uuid"}, "out": {"id": "uuid"}}}},
    {"_path": "b", "service": "payments", "consumes": {"order.placed@v1": {"handler": "onOrderPlaced"}},
     "depends": [{"service": "orders", "method": "getOrder"}]},
]

def test_verify_ok():
    errors, warns = axon.verify(BASE)
    assert errors == [], errors
    assert any("no tiene consumidores" not in w for w in warns) or warns == []

def test_verify_drift():
    orphan = [{"_path": "c", "service": "billing", "consumes": {"ghost@v1": {"handler": "h"}}}]
    errors, _ = axon.verify(BASE + orphan)
    assert any("nadie lo emite" in e for e in errors), errors

    bad_dep = [{"_path": "d", "service": "x", "depends": [{"service": "orders", "method": "nope"}]}]
    errors, _ = axon.verify(BASE + bad_dep)
    assert any("no expone" in e for e in errors), errors

    conflict = [{"_path": "e", "service": "other", "emits": {"order.placed@v1": {"id": "string"}}}]
    errors, _ = axon.verify(BASE + conflict)
    assert any("esquemas distintos" in e for e in errors), errors

def test_build_traceability():
    out = axon.build_ts(axon.load("examples/payments.toml"))
    assert "abstract onOrderPlaced(e: Envelope<OrderPlacedV1>)" in out
    assert "interface CapturePaymentIn" in out
    # el envelope obliga a propagar traza y causalidad
    for field in ("traceparent", "correlationId", "causationId"):
        assert field in out, field
    assert 'newEnvelope("payment.captured@v1", "payments", data, cause)' in out

def test_patterns_enforced():
    out = axon.build_ts(axon.load("examples/payments.toml"))
    # outbox declarado -> el emisor NO toca el bus directamente (sin dual-write)
    assert "this.outbox.stage(newEnvelope" in out
    assert "this.bus.publish(newEnvelope" not in out
    # consumidor idempotente por defecto, no por disciplina
    assert "this.inbox.once(e.id" in out
    assert 'case "order.placed@v1"' in out

def test_infra_derives_dlq_and_secrets():
    tf = axon.build_infra(axon.load_dir("examples"))
    assert '"order.placed.v1.dlq"' in tf and "@" not in tf  # Pub/Sub no admite '@'
    assert "dead_letter_policy" in tf
    assert "payments-stripe-api-key" in tf
    assert "payments-outbox-relay" in tf

def test_diagrams():
    ms = axon.load_dir("examples")
    cls = axon.build_classes(ms)
    assert "+onOrderPlaced(Envelope~OrderPlacedV1~) void" in cls
    assert "PaymentsService ..|> Outbox" in cls

    er = axon.build_er(ms)
    assert "ORDER ||--o{ ORDER_ITEM : order_id" in er
    assert "text provider_ref" in er        # ADD COLUMN plegado
    assert "currency" not in er             # DROP COLUMN plegado

    seq = axon.build_seq(ms, "order.placed@v1")
    assert "orders->>payments: order.placed@v1" in seq
    assert "charges.create (externo)" in seq
    assert "causationId" in seq

def test_migration_checks(tmp=None):
    import pathlib as _p
    d = _p.Path("examples/sql/payments")
    bad = d / "004_oops.sql"
    bad.write_text("ALTER TABLE payment DROP COLUMN status;\n")
    try:
        errors, warns = axon.verify(axon.load_dir("examples"))
        assert any("sin marcar" in e for e in errors), errors
    finally:
        bad.unlink()

    # FK que cruza el limite de servicio
    ms = axon.load_dir("examples")
    f = d / "005_fk.expand.sql"
    f.write_text('ALTER TABLE payment ADD COLUMN o uuid;\n'
                 'CREATE TABLE refund (\n  id uuid PRIMARY KEY,\n'
                 '  order_id uuid REFERENCES "order"(id)\n);\n')
    try:
        errors, _ = axon.verify(axon.load_dir("examples"))
        assert any("cruza el limite" in e for e in errors), errors
    finally:
        f.unlink()

def test_discover_registry():
    reg = axon.registry(axon.load_dir("examples"))
    assert reg["stripe"]["external"] and "charges.create" in reg["stripe"]["methods"]
    assert reg["payments"]["consumes"] == ["order.placed@v1"]

if __name__ == "__main__":
    for name, fn in sorted(globals().items()):
        if name.startswith("test_"):
            fn(); print(f"ok {name}")
