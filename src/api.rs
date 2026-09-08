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
/// The document as of a dated version.
///
/// `at` is what a caller pinned. The shape that applies to it is the OLDEST
/// declared shape at or after that version: a version with no entry for a
/// method did not change it, so what it sees is whatever the next one that did
/// change it promised. That is the same rule the generated adapter chain
/// applies, and if the two disagreed the document would describe an API nobody
/// serves.
pub fn openapi_at(ms: &[Manifest], at: Option<&str>) -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for m in ms {
        for (name, meth) in &m.methods {
            let (Some(verb), Some(path)) = (meth.verb(), meth.path()) else {
                continue;
            };
            // The shape as of the pinned version, when there is one.
            let shape = at.and_then(|v| {
                meth.at
                    .iter()
                    .filter(|(k, _)| k.as_str() >= v)
                    .min_by_key(|(k, _)| k.as_str())
                    .map(|(_, s)| s)
            });
            let input = shape
                .filter(|s| !s.input.is_empty())
                .map(|s| &s.input)
                .unwrap_or(&meth.input);
            let output = shape
                .filter(|s| !s.output.is_empty())
                .map(|s| &s.output)
                .unwrap_or(&meth.output);
            let body = if meth.mutating() {
                json!({"required": true, "content": {"application/json": {"schema": schema(input)}}})
            } else {
                Value::Null
            };
            let mut op = json!({
                "operationId": name,
                "tags": [m.service],
                "responses": {
                    "200": {"description": "ok", "content": {"application/json": {"schema": schema(output)}}},
                    // uniform errors across the whole platform
                    "default": {"description": "error", "content": {"application/problem+json":
                        {"schema": {"$ref": "#/components/schemas/Problem"}}}}
                }
            });
            // Who may call it, in the document a client generator reads. `auth`
            // alone says "somebody"; the scopes say who, and OpenAPI has a
            // place for exactly that.
            match meth.auth.as_deref() {
                Some("required") => {
                    op["security"] = json!([{ "bearer": meth.scopes }]);
                }
                // an explicit empty list is how OpenAPI says "this one is open",
                // and it is not the same as saying nothing
                Some("public") => op["security"] = json!([]),
                _ => {}
            }
            // A retired version says so in the document that gets published,
            // which is the one a client generator reads.
            if meth.deprecated.is_some() {
                op["deprecated"] = json!(true);
                op["x-axon-deprecated"] = json!(meth.deprecated);
            }
            if let Some(su) = &meth.sunset {
                op["x-axon-sunset"] = json!(su);
            }
            if let Some(su) = &meth.successor {
                op["x-axon-successor"] =
                    json!(m.methods.get(su).and_then(|o| o.path()).unwrap_or(su));
            }
            // One response per declared failure. `default` still covers what
            // nobody declared; these say which code arrives and whether trying
            // again can end differently, which is the part a generated client
            // acts on.
            for f in &meth.errors {
                let mut desc = f.code.clone();
                if let Some(d) = &f.detail {
                    desc.push_str(": ");
                    desc.push_str(d);
                }
                op["responses"][f.status.to_string()] = json!({
                    "description": desc,
                    "x-axon-code": f.code,
                    "x-axon-retriable": f.retriable,
                    "content": {"application/problem+json":
                        {"schema": {"$ref": "#/components/schemas/Problem"}}}
                });
            }
            if meth.mutating() {
                op["requestBody"] = body;
                op["parameters"] = json!([{
                    "name": "Idempotency-Key", "in": "header", "required": true,
                    "schema": {"type": "string", "format": "uuid"},
                    "description": "Retrying with the same key does not duplicate the effect."
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
    let api = ms.iter().find(|m| !m.external).map(|m| &m.api);
    // With the header scheme the version is a parameter of every operation, and
    // the answer depends on it: a cache that does not know that serves one
    // caller's shape to another.
    if let Some(api) = api.filter(|a| a.by_header()) {
        for (_, item) in paths.iter_mut() {
            if let Some(ops) = item.as_object_mut() {
                for (_, op) in ops.iter_mut() {
                    let p = json!({
                        "name": api.header_name(), "in": "header", "required": false,
                        "schema": {"type": "string", "enum": api.dates()},
                        "description": "the dated version the caller pins; absent is the default"
                    });
                    match op["parameters"].as_array_mut() {
                        Some(ps) => ps.push(p),
                        None => op["parameters"] = json!([p]),
                    }
                }
            }
        }
    }
    json!({
        "openapi": "3.1.0",
        "info": {"title": "axon",
                 "version": at.map(|v| v.to_string())
                     .or_else(|| api.and_then(|a| a.current().map(|c| c.to_string())))
                     .unwrap_or_else(|| "1.0.0".into()),
                 "description": "Generated from the manifests. Do not edit."},
        "paths": paths,
        "components": {
            // The scheme is named once and every operation points at it. Which
            // scopes exist comes from `[api] scopes`, so a client generator can
            // ask for the right one instead of guessing.
            "securitySchemes": {"bearer": {
                "type": "http", "scheme": "bearer", "bearerFormat": "JWT",
                "description": "validated at the gateway; the service checks the scopes it declares"
            }},
            "schemas": {
            // RFC 7807: one error format across the whole platform
            "Problem": {"type": "object", "required": ["type", "title", "status"], "properties": {
                "type": {"type": "string"}, "title": {"type": "string"},
                "status": {"type": "integer"}, "detail": {"type": "string"},
                "code": {"type": "string", "description": "the code declared in the manifest"},
                "traceId": {"type": "string", "description": "the trace-id from the traceparent"}
            }}
        }}
    })
}

// ---------- tests ----------

fn value(t: &str, k: &str) -> String {
    match t {
        "uuid" => "\"00000000-0000-4000-8000-000000000000\"".into(),
        "timestamp" => "\"2026-01-01T00:00:00.000Z\"".into(),
        "int" | "float" => "1".into(),
        "bool" => "true".into(),
        "money" => "{ amount: 100, currency: \"MXN\" }".into(),
        _ => format!("\"{k}\""),
    }
}

fn fixture(name: &str, kind: &str, campos: &Fields) -> String {
    let body: Vec<String> = campos
        .iter()
        .map(|(k, t)| format!("  {k}: {},", value(t, k)))
        .collect();
    format!(
        "export const {name}: {kind} = {{\n{}\n}};\n",
        body.join("\n")
    )
}

/// A double of the declared dependencies.
///
/// The answers are fixtures derived from the PEER's contract, narrowed to what
/// this service declared it reads. That is what separates it from a mock
/// written by hand: a hand-written one can answer a field the other side does
/// not return, and the test passes right up to production. Here the shape is
/// the contract\'s, and `tsc` holds it.
fn transport_double(ms: &[Manifest], m: &Manifest) -> String {
    if m.depends.is_empty() {
        return String::new();
    }
    let mut answers = Vec::new();
    for d in &m.depends {
        let tgt = d.target();
        let Some(sig) = ms
            .iter()
            .find(|o| o.service == tgt)
            .and_then(|o| o.methods.get(&d.method))
        else {
            continue;
        };
        let mine: Fields = match &d.uses {
            Some(uses) => sig
                .output
                .iter()
                .filter(|(k, _)| uses.iter().any(|u| u == *k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            None => sig.output.clone(),
        };
        let body: Vec<String> = mine
            .iter()
            .map(|(k, t)| format!("{k}: {}", value(t, k)))
            .collect();
        answers.push(format!(
            "  \"{tgt}.{}\": {{ {} }},",
            d.method,
            body.join(", ")
        ));
    }
    format!(
        "/** What each declared dependency answers by default: a fixture of ITS\n \
         *  contract, cut down to what this service declared it reads. */\n\
         export const declaredAnswers: Record<string, unknown> = {{\n{answers}\n}};\n\n\
         /** A transport that answers the contract and records what was asked of it.\n \
         *\n \
         *  `on` replaces one answer and `failWith` makes it fail, which is how the\n \
         *  declared policy gets exercised without a network: a retriable failure has\n \
         *  to arrive 1 + retries times and a final one exactly once. */\n\
         export class FakeTransport implements Transport {{\n  \
           readonly calls: {{ target: string; method: string; body: unknown }}[] = [];\n  \
           readonly #answers = new Map<string, (body: unknown) => unknown>();\n  \
           readonly #failures = new Map<string, unknown>();\n\n  \
           on(target: string, method: string, fn: (body: unknown) => unknown) {{\n    \
             this.#answers.set(`${{target}}.${{method}}`, fn);\n    return this;\n  \
           }}\n\n  \
           failWith(target: string, method: string, err: unknown) {{\n    \
             this.#failures.set(`${{target}}.${{method}}`, err);\n    return this;\n  \
           }}\n\n  \
           /** How many times one method was really called. */\n  \
           timesCalled(target: string, method: string) {{\n    \
             return this.calls.filter((c) => c.target === target && c.method === method).length;\n  \
           }}\n\n  \
           async call(target: string, method: string, body: unknown, _h: Record<string, string>) {{\n    \
             this.calls.push({{ target, method, body }});\n    \
             const key = `${{target}}.${{method}}`;\n    \
             const failure = this.#failures.get(key);\n    \
             if (failure) throw failure;\n    \
             const custom = this.#answers.get(key);\n    \
             if (custom) return custom(body);\n    \
             const declared = declaredAnswers[key];\n    \
             if (declared === undefined) {{\n      \
               throw new Error(`${{key}} is not a declared dependency of {svc}`);\n    \
             }}\n    return declared;\n  \
           }}\n\
         }}\n\n\
         /** The generated clients over the double: the declared timeout, retries and\n \
         *  breaker all running, with no network. */\n\
         export const fakeClients = (t: FakeTransport = new FakeTransport()) =>\n  \
           [new Clients(t), t] as const;\n",
        answers = answers.join("\n"),
        svc = m.service,
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
    if !m.depends.is_empty() {
        tipos.push("Clients".into());
        tipos.push("type Transport".into());
    }
    // The declared failures are projected as three things, and the suite below
    // checks that the three agree.
    if m.methods.values().any(|me| !me.errors.is_empty()) {
        tipos.push("AxonProblem".into());
        tipos.push("declaredErrors".into());
        tipos.push("fail".into());
        tipos.push("problem".into());
    }

    // the schema of a consumed event is declared by its emitter
    let mut consumidos: Vec<(&String, Fields)> = Vec::new();
    for (ev, spec) in &m.consumes {
        let campos = m
            .emits
            .get(ev)
            .or_else(|| ms.iter().find_map(|o| o.emits.get(ev)))
            .ok_or_else(|| format!("{svc}: consumes `{ev}` and nobody was found emitting it"))?;
        // The fixture feeds the HANDLER, and the handler receives what this
        // service declared it reads: the owner's other fields do not exist on
        // this side, so building them would not compile.
        let campos = match spec.uses.as_deref() {
            Some(uses) => campos
                .iter()
                .filter(|(k, _)| uses.iter().any(|u| u == *k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            None => campos.clone(),
        };
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
         //   machineTests();{e}\n\
         import {{ describe, it }} from \"node:test\";\n\
         import assert from \"node:assert/strict\";\n\
         import {{\n{t},\n}} from \"{c}\";\n",
        p = pascal(svc),
        o = if m.patterns.outbox { ", outbox" } else { "" },
        e = if m.methods.values().any(|me| !me.errors.is_empty()) {
            "\n//   errorTests();"
        } else {
            ""
        },
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

    o.push(transport_double(ms, m));
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
    o.push(error_tests(m));
    Ok(o.join("\n"))
}

/// Tests for the declared failures. Pure, like the machine ones: they need
/// nothing from the person's code, because what they check is that the
/// projections of the same declaration agree.
///
/// It is worth generating because the two ends are generated separately —the
/// table and `fail` on one side, the `problem+json` body on the other— and a
/// disagreement between them is exactly the drift that stops the caller from
/// being able to trust `retriable`.
fn error_tests(m: &Manifest) -> String {
    let mut o = vec![
        "/** Tests for the declared failures. They need none of your code. */\nexport function errorTests() {"
            .to_string(),
    ];
    let declaring: Vec<(&String, &Method)> = m
        .methods
        .iter()
        .filter(|(_, me)| !me.errors.is_empty())
        .collect();
    if declaring.is_empty() {
        o.push("  // this service declares no `errors`".into());
        o.push("}\n".into());
        return o.join("\n");
    }
    o.push(format!(
        "  describe(\"{svc} · declared failures\", () => {{\n    \
           it(\"`fail` throws the status and the code the manifest declares\", () => {{\n      \
             for (const [method, failures] of Object.entries(declaredErrors)) {{\n        \
               for (const f of failures) {{\n          \
                 assert.throws(\n            \
                   () => fail(method as keyof typeof declaredErrors, f.code as never),\n            \
                   (err: unknown) => {{\n              \
                     assert.ok(err instanceof AxonProblem, `${{method}}.${{f.code}} is not an AxonProblem`);\n              \
                     assert.equal(err.code, f.code);\n              \
                     assert.equal(err.status, f.status);\n              \
                     return true;\n            \
                   }},\n          \
                 );\n        \
               }}\n      \
             }}\n    }});\n\n    \
           it(\"the body on the wire carries that same code and status\", () => {{\n      \
             for (const [method, failures] of Object.entries(declaredErrors)) {{\n        \
               for (const f of failures) {{\n          \
                 try {{\n            \
                   fail(method as keyof typeof declaredErrors, f.code as never);\n          \
                 }} catch (err) {{\n            \
                   const body = problem(err, newEnvelope(\"test\", \"test\", {{}}));\n            \
                   assert.equal(body.status, f.status, `${{f.code}} goes out with another status`);\n            \
                   assert.equal(body.title, f.code, \"the code does not travel in the body\");\n            \
                   assert.match(body.type, /{svc}/, \"the type does not name the service that failed\");\n            \
                   assert.ok(body.traceId, \"the failure goes out with no trace\");\n          \
                 }}\n        \
               }}\n      \
             }}\n    }});\n\n    \
           it(\"an undeclared failure does not come out looking declared\", () => {{\n      \
             const body = problem(new Error(\"the disk filled up\"));\n      \
             assert.equal(body.status, 500, \"a failure nobody declared came out as something else\");\n      \
             assert.equal(body.title, \"internal\");\n      \
             assert.ok(!(\"code\" in body), \"it invented a code for a failure nobody declared\");\n    \
           }});\n\n    \
           it(\"nothing final is offered as retriable\", () => {{\n      \
             // A 4xx says the request is what is wrong: sending it again ends the\n      \
             // same way, and the caller's client would spend its budget for nothing.\n      \
             const notYet = [408, 425, 429];\n      \
             for (const failures of Object.values(declaredErrors)) {{\n        \
               for (const f of failures) {{\n          \
                 assert.ok(f.status >= 400 && f.status < 600, `${{f.code}} is not a failure`);\n          \
                 if (f.retriable && f.status < 500) {{\n            \
                   assert.ok(notYet.includes(f.status), `${{f.code}} is retriable on ${{f.status}}`);\n          \
                 }}\n        \
               }}\n      \
             }}\n    }});\n  \
         }});",
        svc = m.service
    ));
    o.push("}\n".into());
    o.join("\n")
}
