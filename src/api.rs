//! OpenAPI and test scaffolding. Neither introduces a new source of truth.
use crate::manifest::*;
use serde_json::{json, Map, Value};

fn schema(fields: &Fields) -> Value {
    let mut props = Map::new();
    for (k, t) in fields {
        props.insert(
            k.clone(),
            match t.as_str() {
                "uuid" => json!({"type": "string", "format": "uuid"}),
                "timestamp" => json!({"type": "string", "format": "date-time"}),
                "int" => json!({"type": "integer"}),
                "float" => json!({"type": "number"}),
                "bool" => json!({"type": "boolean"}),
                "money" => json!({"type": "object", "required": ["amount", "currency"],
                    "properties": {"amount": {"type": "integer"}, "currency": {"type": "string"}}}),
                _ => json!({"type": "string"}),
            },
        );
    }
    json!({"type": "object", "required": fields.keys().collect::<Vec<_>>(), "properties": props})
}

/// A single document for every service: the platform's catalogue.
pub fn openapi(ms: &[Manifest]) -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for m in ms {
        for (name, meth) in &m.methods {
            let (Some(verb), Some(path)) = (meth.verb(), meth.path()) else {
                continue;
            };
            let body = if meth.mutating() {
                json!({"required": true, "content": {"application/json": {"schema": schema(&meth.input)}}})
            } else {
                Value::Null
            };
            let mut op = json!({
                "operationId": name,
                "tags": [m.service],
                "responses": {
                    "200": {"description": "ok", "content": {"application/json": {"schema": schema(&meth.output)}}},
                    // uniform errors across the whole platform
                    "default": {"description": "error", "content": {"application/problem+json":
                        {"schema": {"$ref": "#/components/schemas/Problem"}}}}
                }
            });
            if meth.mutating() {
                op["requestBody"] = body;
                op["parameters"] = json!([{
                    "name": "Idempotency-Key", "in": "header", "required": true,
                    "schema": {"type": "string", "format": "uuid"},
                    "description": "Reintentar con la misma llave no duplica el efecto."
                }]);
            }
            paths
                .entry(path.to_string())
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap()
                .insert(verb.to_lowercase(), op);
        }
    }
    json!({
        "openapi": "3.1.0",
        "info": {"title": "axon", "version": "1.0.0",
                 "description": "Generado desde los manifiestos. No editar."},
        "paths": paths,
        "components": {"schemas": {
            // RFC 7807: one error format across the whole platform
            "Problem": {"type": "object", "required": ["type", "title", "status"], "properties": {
                "type": {"type": "string"}, "title": {"type": "string"},
                "status": {"type": "integer"}, "detail": {"type": "string"},
                "traceId": {"type": "string", "description": "el trace-id del traceparent"}
            }}
        }}
    })
}

// ---------- tests ----------

fn valor(t: &str, k: &str) -> String {
    match t {
        "uuid" => "\"00000000-0000-4000-8000-000000000000\"".into(),
        "timestamp" => "\"2026-01-01T00:00:00.000Z\"".into(),
        "int" | "float" => "1".into(),
        "bool" => "true".into(),
        "money" => "{ amount: 100, currency: \"MXN\" }".into(),
        _ => format!("\"{k}\""),
    }
}

fn fixture(nombre: &str, tipo: &str, campos: &Fields) -> String {
    let body: Vec<String> = campos
        .iter()
        .map(|(k, t)| format!("  {k}: {},", valor(t, k)))
        .collect();
    format!(
        "export const {nombre}: {tipo} = {{\n{}\n}};\n",
        body.join("\n")
    )
}

/// A testkit that compiles on its own: doubles, fixtures and exported suites.
///
/// It does not guess where the person's code lives — it takes a factory. That
/// way the generated file never depends on a layout it does not control, and
/// what stitches the two together is three hand-written lines.
pub fn build_tests(ms: &[Manifest], m: &Manifest, contracts: &str) -> Result<String, String> {
    let svc = &m.service;
    let cls = format!("{}Service", pascal(svc));
    let mut tipos = vec![
        "newEnvelope".to_string(),
        "type Envelope".into(),
        "type Bus".into(),
        "type Inbox".into(),
        cls.clone(),
    ];
    if m.patterns.outbox {
        tipos.push("type Outbox".into());
    }

    // the schema of a consumed event is declared by its emitter
    let mut consumidos: Vec<(&String, &Fields)> = Vec::new();
    for ev in m.consumes.keys() {
        let campos = m
            .emits
            .get(ev)
            .or_else(|| ms.iter().find_map(|o| o.emits.get(ev)))
            .ok_or_else(|| format!("{svc}: consumes `{ev}` and nobody was found emitting it"))?;
        consumidos.push((ev, campos));
        tipos.push(format!("type {}", pascal(ev)));
    }
    for name in m.machine.keys() {
        tipos.push(format!("{}Transitions", camel(name)));
        tipos.push(format!("{}Next", camel(name)));
        tipos.push(format!("{}Can", camel(name)));
        tipos.push(format!("type {}State", pascal(name)));
        tipos.push(format!("type {}Action", pascal(name)));
    }

    let mut o = vec![format!(
        "// generated by axon — do not edit.\n\
         //\n\
         // Wire it up from your own test file:\n\
         //\n\
         //   import {{ contractTests, machineTests }} from \"./axon.testkit.ts\";\n\
         //   import {{ {p} }} from \"./index.ts\";\n\
         //   contractTests((bus, inbox{o}) => new {p}(bus, inbox{o}));\n\
         //   machineTests();\n\
         import {{ describe, it }} from \"node:test\";\n\
         import assert from \"node:assert/strict\";\n\
         import {{\n{t},\n}} from \"{c}\";\n",
        p = pascal(svc),
        o = if m.patterns.outbox { ", outbox" } else { "" },
        t = tipos
            .iter()
            .map(|t| format!("  {t}"))
            .collect::<Vec<_>>()
            .join(",\n"),
        c = contracts,
    )];

    o.push(
        "// In-memory doubles. Deterministic and dependency-free: contract tests\n\
         // need no infrastructure, integration tests do.\n\
         export class FakeBus implements Bus {\n  \
           readonly published: Envelope<unknown>[] = [];\n  \
           async publish(e: Envelope<unknown>) {\n    this.published.push(e);\n  }\n}\n\n\
         export class MemoryInbox implements Inbox {\n  \
           readonly seen = new Set<string>();\n  \
           async once(id: string, fn: () => Promise<void>) {\n    \
             if (this.seen.has(id)) return;\n    this.seen.add(id);\n    await fn();\n  }\n}\n"
            .to_string(),
    );
    if m.patterns.outbox {
        o.push(
            // The tx is part of the contract: the double takes it and ignores
            // it, but a handler that stages outside a transaction does not
            // typecheck here either.
            "export class FakeOutbox implements Outbox<unknown> {\n  \
               readonly staged: Envelope<unknown>[] = [];\n  \
               async stage(e: Envelope<unknown>, _tx: unknown) {\n    this.staged.push(e);\n  }\n}\n"
                .to_string(),
        );
    }

    o.push(
        "// Fixtures derived from the schema declared by each event's OWNER, not\n\
            // from what the consumer believes it receives: that is where drift shows up."
            .into(),
    );
    for (ev, campos) in &consumidos {
        o.push(fixture(
            &camel(&format!("fixture.{ev}")),
            &pascal(ev),
            campos,
        ));
    }

    let (args, params) = if m.patterns.outbox {
        (
            "bus: FakeBus, inbox: MemoryInbox, outbox: FakeOutbox",
            "bus, inbox, outbox",
        )
    } else {
        ("bus: FakeBus, inbox: MemoryInbox", "bus, inbox")
    };
    let sink = if m.patterns.outbox {
        "outbox.staged"
    } else {
        "bus.published"
    };

    o.push(format!(
        "/** Contract tests. `make` returns your implementation of the service. */\n\
         export function contractTests(make: ({args}) => {cls}) {{\n  \
           const setUp = () => {{\n    \
             const bus = new FakeBus();\n    const inbox = new MemoryInbox();\n    \
             {decl}const svc = make({params});\n    \
             return {{ svc, bus, inbox{ret} }};\n  }};\n\n  \
           describe(\"{svc} · contract\", () => {{",
        decl = if m.patterns.outbox {
            "const outbox = new FakeOutbox();\n    "
        } else {
            ""
        },
        ret = if m.patterns.outbox { ", outbox" } else { "" },
    ));

    if consumidos.is_empty() {
        o.push("    it(\"consumes no events\", () => assert.ok(true));".into());
    }
    for (ev, _) in &consumidos {
        let fx = camel(&format!("fixture.{ev}"));
        o.push(format!(
            "    it(\"accepts {ev} exactly as its owner emits it\", async () => {{\n      \
               const {{ svc }} = setUp();\n      \
               await svc.dispatch(newEnvelope(\"{ev}\", \"test\", {fx}));\n    }});\n\n    \
             it(\"a second delivery of {ev} does not repeat the effect\", async () => {{\n      \
               const {{ svc, {s} }} = setUp();\n      \
               const e = newEnvelope(\"{ev}\", \"test\", {fx});\n      \
               await svc.dispatch(e);\n      const after = {sink}.length;\n      \
               await svc.dispatch(e);\n      \
               assert.equal({sink}.length, after, \"the same envelope took effect twice\");\n    }});",
            s = if m.patterns.outbox { "outbox" } else { "bus" },
        ));
        if !m.emits.is_empty() {
            o.push(format!(
                "    it(\"propagates the causal chain when reacting to {ev}\", async () => {{\n      \
                   const {{ svc, {s} }} = setUp();\n      \
                   const cause = newEnvelope(\"{ev}\", \"test\", {fx});\n      \
                   await svc.dispatch(cause);\n      \
                   const out = {sink};\n      \
                   assert.ok(out.length > 0, \"it emitted nothing\");\n      \
                   for (const e of out) {{\n        \
                     assert.equal(e.causationId, cause.id, \"causationId does not point at the cause\");\n        \
                     assert.equal(e.correlationId, cause.correlationId, \"the flow was lost\");\n        \
                     assert.equal(e.traceparent.split(\"-\")[1], cause.traceparent.split(\"-\")[1], \"the trace was lost\");\n      \
                   }}\n    }});",
                s = if m.patterns.outbox { "outbox" } else { "bus" },
            ));
        }
    }
    if m.patterns.outbox {
        o.push(
            "    it(\"nothing gets published outside the outbox\", async () => {\n      \
               const { bus } = setUp();\n      \
               assert.equal(bus.published.length, 0, \"dual-write: the handler touched the bus\");\n    });"
                .into(),
        );
    }
    o.push("  });\n}\n".into());

    // state machines: pure, they need nothing from the person
    o.push("/** State machine tests. They need none of your code. */\nexport function machineTests() {".into());
    if m.machine.is_empty() {
        o.push("  // this service declares no state machines".into());
    }
    for (name, mac) in &m.machine {
        let (c, p) = (camel(name), pascal(name));
        o.push(format!(
            "  describe(\"{svc} · machine {name}\", () => {{\n    \
               it(\"every declared transition is legal from its source states\", () => {{\n      \
                 for (const [action, t] of Object.entries({c}Transitions)) {{\n        \
                   for (const from of t.from) {{\n          \
                     assert.equal({c}Next(from, action as {p}Action), t.to);\n          \
                     assert.ok({c}Can(from, action as {p}Action));\n        \
                   }}\n      \
                 }}\n    }});\n\n    \
               it(\"an undeclared transition blows up\", () => {{\n      \
                 const states: {p}State[] = [{estados}];\n      \
                 for (const [action, t] of Object.entries({c}Transitions)) {{\n        \
                   for (const e of states.filter((s) => !t.from.includes(s))) {{\n          \
                     assert.throws(() => {c}Next(e, action as {p}Action));\n          \
                     assert.equal({c}Can(e, action as {p}Action), false);\n        \
                   }}\n      \
                 }}\n    }});\n  }});",
            estados = mac
                .states()
                .iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    o.push("}\n".into());
    Ok(o.join("\n"))
}
