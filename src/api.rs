//! OpenAPI y andamiaje de pruebas. Ninguno introduce una fuente de verdad nueva.
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

/// Un solo documento para todos los servicios: el catalogo de la plataforma.
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
                    // errores uniformes en toda la plataforma
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
            // RFC 7807: un solo formato de error en toda la plataforma
            "Problem": {"type": "object", "required": ["type", "title", "status"], "properties": {
                "type": {"type": "string"}, "title": {"type": "string"},
                "status": {"type": "integer"}, "detail": {"type": "string"},
                "traceId": {"type": "string", "description": "el trace-id del traceparent"}
            }}
        }}
    })
}

// ---------- pruebas ----------

fn fixture(fields: &Fields) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(k, t)| {
            let v = match t.as_str() {
                "uuid" => "\"00000000-0000-4000-8000-000000000000\"".to_string(),
                "timestamp" => "\"2026-01-01T00:00:00.000Z\"".to_string(),
                "int" | "float" => "1".to_string(),
                "bool" => "true".to_string(),
                "money" => "{ amount: 100, currency: \"MXN\" }".to_string(),
                _ => format!("\"{k}\""),
            };
            format!("  {k}: {v},")
        })
        .collect();
    format!("{{\n{}\n}}", body.join("\n"))
}

/// Tres capas, cada una desde una parte distinta del manifiesto:
/// - unitaria: el handler contra una fixture del esquema del EMISOR
/// - integracion: el mismo compose de `--target local`, no un mock
/// - e2e: la cadena causal de `axon seq` como escenario ejecutable
pub fn build_tests(ms: &[Manifest], target: &Manifest) -> String {
    let svc = &target.service;
    let cls = format!("{}Service", pascal(svc));
    let emitters: Vec<(&String, &Fields)> = ms.iter().flat_map(|m| m.emits.iter()).collect();

    let mut o = vec![format!(
        "// generado por axon — andamiaje. Rellena los asserts de negocio.\n\
         import {{ describe, it, expect, beforeAll }} from \"vitest\";\n\
         import type {{ Envelope }} from \"./contracts\";\n\
         import {{ newEnvelope }} from \"./contracts\";\n\
         import {{ {svc} }} from \"./{svc}\";\n"
    )];

    // fixtures: del esquema del emisor, no de lo que el consumidor cree
    o.push("// fixtures derivadas del esquema declarado por el EMISOR de cada evento".into());
    for ev in target.consumes.keys() {
        if let Some((_, fields)) = emitters.iter().find(|(e, _)| *e == ev) {
            o.push(format!(
                "export const {} = {};",
                camel(&format!("fixture.{ev}")),
                fixture(fields)
            ));
        }
    }

    // unitarias
    o.push(format!("\ndescribe(\"{svc} · unitarias\", () => {{"));
    for (ev, spec) in &target.consumes {
        o.push(format!(
            "  it(\"{} acepta {ev} tal como lo emite su dueno\", async () => {{\n    \
             const e = newEnvelope(\"{ev}\", \"test\", {});\n    \
             await new {cls}(bus, inbox).{}(e);\n    \
             expect(bus.published).toHaveLength(1); // TODO: assert de negocio\n  }});",
            spec.handler,
            camel(&format!("fixture.{ev}")),
            spec.handler
        ));
    }
    // el patron que mas se rompe en produccion, probado por defecto
    if let Some((ev, spec)) = target.consumes.iter().next() {
        o.push(format!(
            "  it(\"es idempotente: la segunda entrega no repite el efecto\", async () => {{\n    \
             const e = newEnvelope(\"{ev}\", \"test\", {});\n    \
             const s = new {cls}(bus, inbox);\n    \
             await s.dispatch(e);\n    await s.dispatch(e); // misma id\n    \
             expect(bus.published).toHaveLength(1);\n  }});",
            camel(&format!("fixture.{ev}"))
        ));
        let _ = spec;
    }
    o.push("});".into());

    // integracion contra el compose local
    o.push(format!(
        "\ndescribe(\"{svc} · integracion\", () => {{\n  \
         // infra real, la misma que `axon infra --target local`: nada de mocks de DB\n  \
         beforeAll(async () => {{ await sh(\"docker compose -f axon.local.yml up -d --wait\"); }});\n  \
         it(\"persiste y publica en la misma transaccion\", async () => {{\n    \
         // TODO: caso real contra postgres + NATS\n    expect(true).toBe(true);\n  }});\n}});"
    ));

    // e2e desde la cadena causal declarada
    let roots: Vec<&String> = target.emits.keys().collect();
    if !roots.is_empty() {
        o.push(format!(
            "\ndescribe(\"{svc} · e2e\", () => {{\n  \
             it(\"el flujo causal real coincide con `axon seq`\", async () => {{\n    \
             // publica el evento raiz y compara el arbol de `axon trace` con el esperado\n    \
             const esperado = await sh(\"axon seq {} manifests/\");\n    \
             const real = await sh(\"axon trace .axon/local.ndjson --seq\");\n    \
             expect(normaliza(real)).toEqual(normaliza(esperado));\n  }});\n}});",
            roots[0]
        ));
    }
    o.join("\n")
}
