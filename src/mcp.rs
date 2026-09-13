//! `axon mcp` — the compiler as a tool an agent can pick up.
//!
//! What a model is good at is the part axon does not do: deciding where the
//! service boundary goes, what an event is called, which guarantee the domain
//! needs. What it is bad at is knowing whether what it just wrote holds up —
//! and that is exactly what `verify` answers.
//!
//! So this exposes the compiler and nothing else. No model lives in here: a
//! compiler that calls an API stops being deterministic, needs a key, and
//! cannot be the thing that settles an argument.
//!
//! JSON-RPC over stdio, like the language server —but NOT the same framing.
//! The LSP wraps every message in a `Content-Length` header; MCP sends one
//! JSON object per line. They look close enough to reuse by mistake, and a
//! client that speaks the other one just waits until it times out.
use crate::lsp::{schema, values};
use crate::{emit, manifest};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;

/// The version this speaks. A client that asks for another one gets its own
/// back when we can serve it: the handshake is where that is negotiated, and
/// refusing over a number nobody reads helps nobody.
const PROTOCOL: &str = "2025-06-18";

/// One JSON object per line, in and out. Nothing else: a line that is not a
/// message —a log, a warning from a library— corrupts the stream, which is why
/// nothing here ever writes to stdout on its own.
fn send(v: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn reply(id: Option<Value>, result: Value) {
    send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

pub fn serve() -> Result<(), String> {
    let stdin = std::io::stdin().lock();
    for line in stdin.lines().map_while(Result::ok) {
        let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL);
                reply(
                    id,
                    json!({
                        "protocolVersion": asked,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "axon", "version": env!("CARGO_PKG_VERSION") },
                        "instructions": INSTRUCTIONS,
                    }),
                );
            }
            "tools/list" => reply(id, json!({ "tools": tools() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                let (text, failed) = match call(name, &args) {
                    Ok(t) => (t, false),
                    // an error goes back INSIDE the result and not as a
                    // protocol error: the model has to see what went wrong to
                    // fix it, and a transport error it never reads is a model
                    // that repeats the same call
                    Err(e) => (e, true),
                };
                reply(
                    id,
                    json!({ "content": [{ "type": "text", "text": text }], "isError": failed }),
                );
            }
            "ping" => reply(id, json!({})),
            // notifications carry no id and want no answer
            _ => {
                if id.is_some() {
                    send(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": format!("no method `{method}`") }
                    }));
                }
            }
        }
    }
    Ok(())
}

/// What the client is told before it calls anything. The order matters more
/// than the list: a model that writes a whole platform and verifies at the end
/// has to unpick everything at once.
const INSTRUCTIONS: &str = "\
axon compiles manifests into contracts, infrastructure and diagrams, and refuses the \
ones that contradict themselves. The design is yours; the verdict is its.

The loop that works: `manifest_schema` to see what can be declared and which values a \
key accepts, write ONE service, `verify` it, fix what it says, then the next one. \
Verifying at the end means unpicking everything at once.

`verify` is the one that matters. Its findings are sentences that say what breaks and \
why, and they are meant to be acted on, not summarised to the user. An error is a \
refusal; a warning is a decision somebody has to take.

What axon will not tell you: where the service boundary goes, or what an event should \
be called. That is the part you are for.";

fn tools() -> Vec<Value> {
    let path = json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "a manifest, or a directory of them" }
        },
        "required": ["path"]
    });
    vec![
        json!({
            "name": "verify",
            "description": "Every rule over the manifests: an event nobody consumes, a \
                guarantee the topology contradicts, a retry against something that is not \
                idempotent, an FK crossing a service boundary, a scope with a typo. \
                Returns the findings as JSON. Run it after every service you write.",
            "inputSchema": path,
        }),
        json!({
            "name": "graph",
            "description": "The event topology as mermaid: who emits what and who reads \
                it, and which calls are synchronous. Read it to see the shape of what is \
                declared before changing it.",
            "inputSchema": path,
        }),
        json!({
            "name": "manifest_schema",
            "description": "What a manifest can declare: the blocks, their keys, and the \
                closed list of values a key accepts. It comes out of the compiler's own \
                model, so it is what this version really understands —ask before writing \
                a manifest, instead of guessing a key that will be refused.",
            "inputSchema": { "type": "object", "properties": {} },
        }),
        json!({
            "name": "contracts",
            "description": "The TypeScript a manifest generates: the types of its events \
                and methods, the client with its timeout and its breaker, the base class \
                to implement. Shows what writing a manifest actually buys.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "manifest": { "type": "string", "description": "the service's manifest" },
                    "peers": { "type": "string", "description": "directory with the others; \
                        that is where the type of what it consumes comes from" }
                },
                "required": ["manifest"]
            },
        }),
    ]
}

fn call(name: &str, args: &Value) -> Result<String, String> {
    let path = || {
        args.get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "`path` is required".to_string())
    };
    match name {
        "verify" => {
            let p = path()?;
            let ms = manifest::discover(&[p.to_string()])?;
            let root = Path::new(p);
            let root = if root.is_dir() {
                root
            } else {
                root.parent().unwrap_or(Path::new("."))
            };
            let r = crate::full_report(&ms, root);
            serde_json::to_string_pretty(&json!({
                "ok": r.errors.is_empty(),
                "services": ms.len(),
                "errors": r.errors,
                "warnings": r.warnings,
            }))
            .map_err(|e| e.to_string())
        }
        "graph" => Ok(emit::build_graph(&manifest::discover(&[
            path()?.to_string()
        ])?)),
        "manifest_schema" => Ok(described()),
        "contracts" => {
            let m = args
                .get("manifest")
                .and_then(Value::as_str)
                .ok_or_else(|| "`manifest` is required".to_string())?;
            let peers = args.get("peers").and_then(Value::as_str);
            let all = match peers {
                Some(p) => manifest::discover(&[p.to_string()])?,
                None => vec![],
            };
            emit::build_ts(&manifest::load_any(m)?, &all)
        }
        other => Err(format!("no tool `{other}`")),
    }
}

/// The model's own vocabulary, walked: every block with its keys, and the
/// closed list where a key has one. Derived and not written down, so it cannot
/// describe a version of the manifest that no longer exists.
fn described() -> String {
    let mut out = String::from(
        "What a manifest can declare, from this axon's own model.\n\
         A key with a closed list shows it; anything else takes free text or a number.\n\n",
    );
    walk(schema(), "", &mut out);
    // The blocks with no fixed keys. They cannot come out of the walk —there
    // is nothing to list— and they are the ones a manifest is mostly made of,
    // so leaving them out is worse than saying it in a paragraph.
    out.push_str(
        "Field maps: blocks whose keys are the names YOU choose, and whose values are types.\n\
         \n\
         [emits.\"order.placed@v1\"]   an event, named with its version\n\
         [methods.<name>] in / out    the request and the response\n\
         \n\
         The types: uuid, timestamp, int, float, bool, money —an amount and a currency—\n\
         and anything else is a string. A field is `name = \"type\"`.\n",
    );
    out
}

fn walk(node: &Value, path: &str, out: &mut String) {
    let Some(block) = node.as_object() else {
        return;
    };
    let nested = |v: &Value| v.is_object() || v.as_array().is_some_and(|a| !a.is_empty());

    // The block's OWN keys first, and the blocks under it afterwards. Walking
    // in one pass puts whatever is alphabetically after a nested block under
    // that block's header, which reads as a key of the wrong thing.
    let own: Vec<_> = block.iter().filter(|(_, v)| !nested(v)).collect();
    if !path.is_empty() && !own.is_empty() {
        out.push_str(&format!("[{path}]\n"));
    }
    for (key, _) in &own {
        let accepted = values(path, key, node);
        if accepted.is_empty() {
            out.push_str(&format!("  {key}\n"));
        } else {
            out.push_str(&format!("  {key} = {}\n", accepted.join(" | ")));
        }
    }
    if !own.is_empty() {
        out.push('\n');
    }

    for (key, value) in block.iter().filter(|(_, v)| nested(v)) {
        // A map's entry is named by whoever writes it; what is worth saying is
        // the shape every entry has. The seed names them `x` —and `x@v1` where
        // the name is an event, which carries its version.
        let name = if key == "x" || key.starts_with("x@") {
            "<name>"
        } else {
            key
        };
        let child = format!(
            "{}{}",
            if path.is_empty() {
                String::new()
            } else {
                format!("{path}.")
            },
            name
        );
        match value {
            Value::Object(_) => walk(value, &child, out),
            Value::Array(items) => match items.first() {
                Some(first) if first.is_object() => walk(first, &child, out),
                _ => out.push_str(&format!("  {key} = [...]\n")),
            },
            // the filter above left only what nests
            _ => {}
        }
    }
}
