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

/// Testkit que compila por si solo: dobles, fixtures y suites exportadas.
///
/// No adivina donde vive el codigo de la persona — le pasa una fabrica. Asi el
/// archivo generado nunca depende de un layout que no controla, y el que teje
/// las dos partes son tres lineas escritas a mano.
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

    // el esquema de un evento consumido lo declara su emisor
    let mut consumidos: Vec<(&String, &Fields)> = Vec::new();
    for ev in m.consumes.keys() {
        let campos = m
            .emits
            .get(ev)
            .or_else(|| ms.iter().find_map(|o| o.emits.get(ev)))
            .ok_or_else(|| format!("{svc}: consume `{ev}` y no se encontro quien lo emite"))?;
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
        "// generado por axon — no editar.\n\
         //\n\
         // Enchufalo desde tu propio archivo de pruebas:\n\
         //\n\
         //   import {{ pruebasDeContrato, pruebasDeMaquinas }} from \"./axon.testkit.ts\";\n\
         //   import {{ {p} }} from \"./index.ts\";\n\
         //   pruebasDeContrato((bus, inbox{o}) => new {p}(bus, inbox{o}));\n\
         //   pruebasDeMaquinas();\n\
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
        "// Dobles en memoria. Deterministas y sin dependencias: las pruebas de\n\
         // contrato no necesitan infraestructura, las de integracion si.\n\
         export class BusFalso implements Bus {\n  \
           readonly publicados: Envelope<unknown>[] = [];\n  \
           async publish(e: Envelope<unknown>) {\n    this.publicados.push(e);\n  }\n}\n\n\
         export class InboxEnMemoria implements Inbox {\n  \
           readonly vistos = new Set<string>();\n  \
           async once(id: string, fn: () => Promise<void>) {\n    \
             if (this.vistos.has(id)) return;\n    this.vistos.add(id);\n    await fn();\n  }\n}\n"
            .to_string(),
    );
    if m.patterns.outbox {
        o.push(
            "export class OutboxFalso implements Outbox {\n  \
               readonly guardados: Envelope<unknown>[] = [];\n  \
               async stage(e: Envelope<unknown>) {\n    this.guardados.push(e);\n  }\n}\n"
                .to_string(),
        );
    }

    o.push(
        "// Fixtures derivadas del esquema que declara el DUENO de cada evento,\n\
            // no de lo que el consumidor cree recibir: ahi es donde aparece el drift."
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
            "bus: BusFalso, inbox: InboxEnMemoria, outbox: OutboxFalso",
            "bus, inbox, outbox",
        )
    } else {
        ("bus: BusFalso, inbox: InboxEnMemoria", "bus, inbox")
    };
    let sink = if m.patterns.outbox {
        "outbox.guardados"
    } else {
        "bus.publicados"
    };

    o.push(format!(
        "/** Pruebas de contrato. `crear` devuelve tu implementacion del servicio. */\n\
         export function pruebasDeContrato(crear: ({args}) => {cls}) {{\n  \
           const montar = () => {{\n    \
             const bus = new BusFalso();\n    const inbox = new InboxEnMemoria();\n    \
             {decl}const svc = crear({params});\n    \
             return {{ svc, bus, inbox{ret} }};\n  }};\n\n  \
           describe(\"{svc} · contrato\", () => {{",
        decl = if m.patterns.outbox {
            "const outbox = new OutboxFalso();\n    "
        } else {
            ""
        },
        ret = if m.patterns.outbox { ", outbox" } else { "" },
    ));

    if consumidos.is_empty() {
        o.push("    it(\"no consume eventos\", () => assert.ok(true));".into());
    }
    for (ev, _) in &consumidos {
        let fx = camel(&format!("fixture.{ev}"));
        o.push(format!(
            "    it(\"acepta {ev} tal como lo emite su dueno\", async () => {{\n      \
               const {{ svc }} = montar();\n      \
               await svc.dispatch(newEnvelope(\"{ev}\", \"prueba\", {fx}));\n    }});\n\n    \
             it(\"la segunda entrega de {ev} no repite el efecto\", async () => {{\n      \
               const {{ svc, {s} }} = montar();\n      \
               const e = newEnvelope(\"{ev}\", \"prueba\", {fx});\n      \
               await svc.dispatch(e);\n      const despues = {sink}.length;\n      \
               await svc.dispatch(e);\n      \
               assert.equal({sink}.length, despues, \"el mismo envelope tuvo efecto dos veces\");\n    }});",
            s = if m.patterns.outbox { "outbox" } else { "bus" },
        ));
        if !m.emits.is_empty() {
            o.push(format!(
                "    it(\"propaga la cadena causal al reaccionar a {ev}\", async () => {{\n      \
                   const {{ svc, {s} }} = montar();\n      \
                   const causa = newEnvelope(\"{ev}\", \"prueba\", {fx});\n      \
                   await svc.dispatch(causa);\n      \
                   const salida = {sink};\n      \
                   assert.ok(salida.length > 0, \"no emitio nada\");\n      \
                   for (const e of salida) {{\n        \
                     assert.equal(e.causationId, causa.id, \"causationId no apunta a la causa\");\n        \
                     assert.equal(e.correlationId, causa.correlationId, \"se perdio el flujo\");\n        \
                     assert.equal(e.traceparent.split(\"-\")[1], causa.traceparent.split(\"-\")[1], \"se perdio la traza\");\n      \
                   }}\n    }});",
                s = if m.patterns.outbox { "outbox" } else { "bus" },
            ));
        }
    }
    if m.patterns.outbox {
        o.push(
            "    it(\"nada se publica fuera del outbox\", async () => {\n      \
               const { bus } = montar();\n      \
               assert.equal(bus.publicados.length, 0, \"dual-write: el handler toco el bus\");\n    });"
                .into(),
        );
    }
    o.push("  });\n}\n".into());

    // maquinas de estado: puras, no necesitan nada de la persona
    o.push("/** Pruebas de las maquinas de estado. No necesitan tu codigo. */\nexport function pruebasDeMaquinas() {".into());
    if m.machine.is_empty() {
        o.push("  // este servicio no declara maquinas de estado".into());
    }
    for (name, mac) in &m.machine {
        let (c, p) = (camel(name), pascal(name));
        o.push(format!(
            "  describe(\"{svc} · maquina {name}\", () => {{\n    \
               it(\"cada transicion declarada es legal desde sus estados de origen\", () => {{\n      \
                 for (const [accion, t] of Object.entries({c}Transitions)) {{\n        \
                   for (const desde of t.from) {{\n          \
                     assert.equal({c}Next(desde, accion as {p}Action), t.to);\n          \
                     assert.ok({c}Can(desde, accion as {p}Action));\n        \
                   }}\n      \
                 }}\n    }});\n\n    \
               it(\"una transicion no declarada revienta\", () => {{\n      \
                 const estados: {p}State[] = [{estados}];\n      \
                 for (const [accion, t] of Object.entries({c}Transitions)) {{\n        \
                   for (const e of estados.filter((s) => !t.from.includes(s))) {{\n          \
                     assert.throws(() => {c}Next(e, accion as {p}Action));\n          \
                     assert.equal({c}Can(e, accion as {p}Action), false);\n        \
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
