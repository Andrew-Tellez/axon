//! `axon lsp` — what `axon verify` already knows, inside the editor.
//!
//! The manifests are plain TOML, so the editor already highlights them; what it
//! cannot see is the half that lives between files — an event nobody consumes,
//! a guarantee the topology contradicts, a migration that does not match. That
//! is `verify`, and this publishes it as diagnostics.
//!
//! The whole workspace gets re-read on open and on save, because a rule that
//! crosses services cannot be checked from one buffer. No incremental sync, no
//! in-memory documents: the findings are about what is on disk anyway.
use crate::manifest::{self, Manifest};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub fn serve() -> Result<(), String> {
    let mut stdin = std::io::stdin().lock();
    let mut root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // what was published last round: a file whose findings are all fixed has to
    // be told it is clean, or the editor keeps showing them forever
    let mut published: HashSet<String> = HashSet::new();
    // Diagnostics come off the disk, but completion cannot: what it has to
    // answer about is the half-typed line that has not been saved yet.
    let mut open: std::collections::HashMap<String, String> = std::collections::HashMap::new();

    while let Some(msg) = read_message(&mut stdin) {
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => {
                if let Some(r) = workspace_root(&params) {
                    root = r;
                }
                reply(
                    id,
                    json!({
                        "capabilities": {
                            // full sync: the buffer is what completion answers
                            // about, and a manifest is small enough that
                            // sending it whole costs nothing
                            "textDocumentSync": {
                                "openClose": true,
                                "change": 1,
                                "save": { "includeText": false }
                            },
                            "definitionProvider": true,
                            "hoverProvider": true,
                            "referencesProvider": true,
                            "completionProvider": {
                                // the quote is where a value starts, the dot
                                // opens a nested block
                                "triggerCharacters": ["\"", ".", "["]
                            }
                        },
                        "serverInfo": { "name": "axon", "version": env!("CARGO_PKG_VERSION") }
                    }),
                );
            }
            "textDocument/didOpen" | "textDocument/didSave" => {
                let uri = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if let Some(t) = params.pointer("/textDocument/text").and_then(Value::as_str) {
                    open.insert(uri.clone(), t.to_string());
                }
                published = publish(&root, uri_to_path(&uri).as_deref(), &published);
            }
            "textDocument/didChange" => {
                // full sync: the last change carries the whole document
                if let (Some(uri), Some(text)) = (
                    params.pointer("/textDocument/uri").and_then(Value::as_str),
                    params
                        .pointer("/contentChanges")
                        .and_then(Value::as_array)
                        .and_then(|c| c.last())
                        .and_then(|c| c.get("text"))
                        .and_then(Value::as_str),
                ) {
                    open.insert(uri.to_string(), text.to_string());
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str) {
                    open.remove(uri);
                }
            }
            "textDocument/completion" => {
                let text = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .and_then(|u| open.get(u))
                    .map(String::as_str)
                    .unwrap_or_default();
                let line = params.pointer("/position/line").and_then(Value::as_u64);
                let ch = params
                    .pointer("/position/character")
                    .and_then(Value::as_u64);
                let items = match (line, ch) {
                    (Some(l), Some(c)) => complete(text, l as usize, c as usize),
                    _ => vec![],
                };
                reply(id, json!({ "isIncomplete": false, "items": items }));
            }
            "textDocument/definition" => {
                let text = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .and_then(|u| open.get(u))
                    .map(String::as_str)
                    .unwrap_or_default();
                let at = (
                    params.pointer("/position/line").and_then(Value::as_u64),
                    params
                        .pointer("/position/character")
                        .and_then(Value::as_u64),
                );
                let found = match at {
                    (Some(l), Some(c)) => {
                        let (l, c) = (l as usize, c as usize);
                        token_at(text, l, c).and_then(|t| {
                            define(&root, &t).or_else(|| {
                                // a bare method name: `[[depends]]` names the
                                // service on its own line, above this one
                                let svc = service_above(text, l)?;
                                define(&root, &format!("{svc}.{t}"))
                            })
                        })
                    }
                    _ => None,
                };
                reply(id, found.unwrap_or(Value::Null));
            }
            "textDocument/references" => {
                let text = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .and_then(|u| open.get(u))
                    .map(String::as_str)
                    .unwrap_or_default();
                let found = match (
                    params.pointer("/position/line").and_then(Value::as_u64),
                    params
                        .pointer("/position/character")
                        .and_then(Value::as_u64),
                ) {
                    (Some(l), Some(c)) => token_at(text, l as usize, c as usize)
                        .map(|t| references(&root, &t))
                        .unwrap_or_default(),
                    _ => vec![],
                };
                reply(id, Value::Array(found));
            }
            "textDocument/hover" => {
                let text = params
                    .pointer("/textDocument/uri")
                    .and_then(Value::as_str)
                    .and_then(|u| open.get(u))
                    .map(String::as_str)
                    .unwrap_or_default();
                let found = match (
                    params.pointer("/position/line").and_then(Value::as_u64),
                    params
                        .pointer("/position/character")
                        .and_then(Value::as_u64),
                ) {
                    (Some(l), Some(c)) => hover(&root, text, l as usize, c as usize),
                    _ => None,
                };
                reply(id, found.unwrap_or(Value::Null));
            }
            "shutdown" => reply(id, Value::Null),
            "exit" => break,
            // a request that goes unanswered hangs the client; a notification
            // that is not ours is not our problem
            _ => {
                if id.is_some() {
                    reply(id, Value::Null);
                }
            }
        }
    }
    Ok(())
}

/// Runs the report and publishes it, one notification per file. Returns the
/// URIs that carry findings, so the next round can clear the ones that stop.
fn publish(root: &Path, current: Option<&Path>, previous: &HashSet<String>) -> HashSet<String> {
    let mut by_file: Vec<(PathBuf, Vec<Value>)> = Vec::new();
    for (path, diag) in diagnose(root, current) {
        match by_file.iter_mut().find(|(p, _)| *p == path) {
            Some((_, v)) => v.push(diag),
            None => by_file.push((path, vec![diag])),
        }
    }

    let mut now = HashSet::new();
    for (path, diags) in by_file {
        let uri = path_to_uri(&path);
        notify(
            "textDocument/publishDiagnostics",
            json!({ "uri": uri, "diagnostics": diags }),
        );
        now.insert(uri);
    }
    for gone in previous.difference(&now) {
        notify(
            "textDocument/publishDiagnostics",
            json!({ "uri": gone, "diagnostics": [] }),
        );
    }
    now
}

/// Every finding, placed on a file and a line.
fn diagnose(root: &Path, current: Option<&Path>) -> Vec<(PathBuf, Value)> {
    let sources = vec![root.display().to_string()];
    let ms = match manifest::discover(&sources) {
        Ok(ms) => ms,
        // A manifest that does not parse stops the whole report: that one error
        // IS the report, and it already names its file
        Err(e) => {
            let (path, _) = split_owner(&e);
            let file = path
                .map(|p| root.join(p))
                .filter(|p| p.is_file())
                .or_else(|| current.map(PathBuf::from))
                .unwrap_or_else(|| root.join("."));
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            let (line, len) = locate(&text, &e);
            return vec![(file, diagnostic(&e, line, len, 1))];
        }
    };

    let mut r = crate::full_report(&ms, root);
    // An accepted warning is a decision somebody already took; showing it again
    // on every save is how a suppressions file gets ignored.
    if let Some(a) = crate::accepted::cargar(root) {
        r.warnings.retain(|w| !a.warnings.contains(w));
    }

    let mut out = Vec::new();
    for (message, severity) in r
        .errors
        .iter()
        .map(|e| (e, 1))
        .chain(r.warnings.iter().map(|w| (w, 2)))
    {
        let file = file_for(&ms, root, message).or_else(|| current.map(PathBuf::from));
        let Some(file) = file else { continue };
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let (line, len) = locate(&text, message);
        out.push((file, diagnostic(message, line, len, severity)));
    }
    out
}

fn diagnostic(message: &str, line: u32, len: u32, severity: u8) -> Value {
    json!({
        "range": {
            "start": { "line": line, "character": 0 },
            "end": { "line": line, "character": len }
        },
        "severity": severity,
        "source": "axon",
        "message": message
    })
}

/// The file a finding belongs to. Findings read `service: what happened`, and
/// a manifest remembers where it was loaded from.
fn file_for(ms: &[Manifest], root: &Path, message: &str) -> Option<PathBuf> {
    let (owner, _) = split_owner(message);
    let owner = owner?;
    if let Some(m) = ms.iter().find(|m| m.service == owner) {
        return Some(m.origin.clone());
    }
    // some findings name a path instead: a migration, an included fragment
    let p = root.join(owner);
    p.is_file().then_some(p)
}

/// Splits `owner: rest` off a finding, seeing through a plugin's `[bin] `
/// prefix. Returns none when the message does not carry one.
fn split_owner(message: &str) -> (Option<&str>, &str) {
    let m = match message.split_once("] ") {
        Some((tag, rest)) if tag.starts_with('[') => rest,
        _ => message,
    };
    match m.split_once(':') {
        // a colon inside a sentence is not an owner
        Some((owner, rest)) if !owner.contains(' ') && !owner.is_empty() => (Some(owner), rest),
        _ => (None, m),
    }
}

/// The line a finding points at, and how wide it is.
///
/// The findings are sentences, not spans: what they do carry is the offending
/// thing in backticks. Looking that up in the file is what turns a message into
/// a place, and landing on line 1 when it does not match is no worse than what
/// a terminal already gives.
fn locate(text: &str, message: &str) -> (u32, u32) {
    // a TOML parse error already knows where it is, and says so in words
    if let Some(n) = message
        .split_once("at line ")
        .and_then(|(_, rest)| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|d| d.parse::<u32>().ok())
        .filter(|n| *n > 0)
    {
        let len = text
            .lines()
            .nth(n as usize - 1)
            .map_or(0, |l| l.chars().count());
        return (n - 1, len as u32);
    }
    let quoted: Vec<&str> = message.split('`').skip(1).step_by(2).collect();
    // verbatim first: `emits."order.placed@v1"` is written the same way in the
    // file, and matches the one line that declares it
    for token in &quoted {
        if let Some((i, l)) = text.lines().enumerate().find(|(_, l)| l.contains(*token)) {
            return (i as u32, l.chars().count() as u32);
        }
    }
    // otherwise the key it starts with: `runtime = "nope"` is not in the file
    // verbatim, but `runtime` is the line that put it there
    for token in &quoted {
        let key: String = token
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if key.is_empty() {
            continue;
        }
        let hit = text.lines().enumerate().find(|(_, raw)| {
            let l = raw.trim_start();
            let after = l.strip_prefix(&key).map(str::trim_start);
            after.is_some_and(|a| a.starts_with('='))
                || l.starts_with(&format!("[{key}]"))
                || l.starts_with(&format!("[{key}."))
        });
        if let Some((i, l)) = hit {
            return (i as u32, l.chars().count() as u32);
        }
    }
    (
        0,
        text.lines().next().map_or(0, |l| l.chars().count()) as u32,
    )
}

// ---------- completion ----------

/// A manifest that declares one of everything, so the model can be asked what
/// its keys are instead of being copied into a table here. A field that gets
/// renamed renames in the completion too, and a table would go stale in a week.
///
/// The entries of the blocks that are maps are all called `x`: a `[env.prod]`
/// has the keys of whatever `[env.x]` has, because they are the same type.
const SEED: &str = r#"
service = "x"
[cap]
[api]
[patterns]
[pooler]
[analytics]
[auth]
[cache]
[cache.x]
of = "x"
[search]
[search.x]
of = "x"
key = "x"
[infra]
[infra.buckets.x]
[env.x]
[methods.x]
[consumes."x@v1"]
handler = "x"
[flags.x]
[machine.x]
initial = "x"
[saga.x]
[aggregate.x]
[view.x]
[catalog.x]
key = "x"
fields = { x = "string" }
entries = [{ x = "x" }]
[crud.x]
table = "x"
key = "x"
path = "/x"
[metrics.x]
[rules.x]
metric = "x"
"#;

/// The name every seeded map entry carries.
const ENTRY: &str = "x";

fn schema() -> &'static Value {
    static S: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        let m: Manifest = toml::from_str(SEED).expect("the seed manifest parses");
        serde_json::to_value(m).unwrap_or(Value::Null)
    })
}

/// What to offer at a position of a buffer.
fn complete(text: &str, line: usize, character: usize) -> Vec<Value> {
    let current = text.lines().nth(line).unwrap_or("");
    let prefix: String = current.chars().take(character).collect();
    // inside a `[block]` header there is nothing to say: the name of a service's
    // event or method is the author's to invent
    if prefix.trim_start().starts_with('[') {
        return vec![];
    }
    let path = block_at(text, line);
    let Some(block) = resolve(schema(), &path) else {
        return vec![];
    };
    match prefix.split_once('=') {
        Some((key, written)) => values(&path, key.trim(), block)
            .into_iter()
            // the quote is already there when the editor triggered on it
            .map(|v| {
                let bare = written.contains('"') || v == "true" || v == "false";
                let insert = if bare {
                    v.to_string()
                } else {
                    format!("\"{v}\"")
                };
                item(v, &insert, 12, "value")
            })
            .collect(),
        None => block
            .as_object()
            .map(|o| {
                o.iter()
                    .filter(|(k, _)| *k != ENTRY)
                    .map(|(k, v)| {
                        let table = v.is_object() || v.is_array();
                        let kind = if table { 9 } else { 5 };
                        item(k, k, kind, if table { "block" } else { "key" })
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn item(label: &str, insert: &str, kind: u8, detail: &str) -> Value {
    json!({ "label": label, "insertText": insert, "kind": kind, "detail": detail })
}

/// The block a line is inside: the nearest `[header]` above it.
fn block_at(text: &str, line: usize) -> String {
    let above: Vec<&str> = text.lines().take(line + 1).collect();
    above
        .iter()
        .rev()
        .find_map(|l| {
            let t = l.trim();
            let inner = t
                .strip_prefix("[[")
                .and_then(|r| r.strip_suffix("]]"))
                .or_else(|| t.strip_prefix('[').and_then(|r| r.strip_suffix(']')))?;
            Some(inner.replace('"', ""))
        })
        .unwrap_or_default()
}

/// Walks a block path down the schema. A segment that is not a declared key is
/// the name of a map entry, and every entry of a map has the same shape.
fn resolve<'a>(schema: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = schema;
    if path.is_empty() {
        return Some(cur);
    }
    for seg in path.split('.') {
        cur = cur.get(seg).or_else(|| cur.get(ENTRY))?;
        if let Some(first) = cur.as_array().and_then(|a| a.first()) {
            cur = first;
        }
    }
    Some(cur)
}

/// The values a key accepts, when they are a closed list. The lists live where
/// the rules that enforce them live, so there is one copy of each.
fn values(path: &str, key: &str, block: &Value) -> Vec<&'static str> {
    use crate::manifest as m;
    let head = path.split('.').next().unwrap_or("");
    let list: &[&str] = match (head, key) {
        ("infra" | "env", "state") => &m::ENGINES,
        ("infra" | "env", "runtime") => &m::RUNTIMES,
        ("cap", "consistency") => &["strong", "eventual"],
        ("cap", "on_partition") => &["reject", "degrade"],
        ("api", "versioning") => &["path", "header"],
        ("pooler", "engine") => &["none", "pgdog"],
        ("pooler", "mode") => &["transaction", "session", "statement"],
        ("analytics", "warehouse") => &m::WAREHOUSES,
        ("analytics", "pii") => &["exclude", "hash"],
        ("auth", "verify") => &m::AUTH_VERIFY,
        ("auth", "revocation") => &m::AUTH_REVOCATION,
        ("cache", "engine") => &m::CACHE_ENGINES,
        ("cache", "strategy") => &m::CACHE_STRATEGIES,
        ("search", "engine") => &m::SEARCH_ENGINES,
        ("metrics", "kind") => &m::AGGREGATIONS,
        ("metrics", "window") => &m::WINDOWS,
        ("rules", "comparison") => &m::COMPARISONS,
        ("crud", "operations") => &m::CRUD_OPERATIONS,
        // a boolean does not need a list: the model already says it is one
        _ => match block.get(key) {
            Some(Value::Bool(_)) => &["true", "false"],
            _ => &[],
        },
    };
    list.to_vec()
}

// ---------- hover ----------

/// What is worth saying about the name under the cursor, which is what the
/// file it is in cannot say: who emits the event and who else reads it, what a
/// service promises, which values a key accepts. The prose about a field lives
/// in the model's own doc comments and does not reach the binary, so this says
/// what can be checked instead of what can be quoted.
fn hover(root: &Path, text: &str, line: usize, character: usize) -> Option<Value> {
    let token = token_at(text, line, character)?;
    let ms = manifest::discover(&[root.display().to_string()]).unwrap_or_default();
    let md = event_hover(&ms, &token)
        .or_else(|| service_hover(&ms, &token))
        .or_else(|| key_hover(text, line, &token))
        // on a value, say what its key accepts
        .or_else(|| {
            let key = text
                .lines()
                .nth(line)?
                .split('=')
                .next()?
                .trim()
                .to_string();
            key_hover(text, line, &key)
        })?;
    Some(json!({ "contents": { "kind": "markdown", "value": md } }))
}

fn event_hover(ms: &[Manifest], token: &str) -> Option<String> {
    let m = ms.iter().find(|m| m.emits.contains_key(token))?;
    let readers: Vec<&str> = ms
        .iter()
        .filter(|o| o.consumes.contains_key(token))
        .map(|o| o.service.as_str())
        .collect();
    let mut out = format!("**{token}**\n\nEmitted by `{}`", m.service);
    // an event nobody reads is a rule of `verify`, and seeing it here is
    // seeing it before the commit
    out.push_str(&match readers.len() {
        0 => " · consumed by nobody".to_string(),
        _ => format!(" · consumed by `{}`", readers.join("`, `")),
    });
    out.push_str("\n\n");
    for (field, ty) in &m.emits[token] {
        let pii = if m.pii.contains(field) { " · pii" } else { "" };
        out.push_str(&format!("- `{field}`: {ty}{pii}\n"));
    }
    Some(out)
}

fn service_hover(ms: &[Manifest], token: &str) -> Option<String> {
    let m = ms.iter().find(|m| m.service == token)?;
    let cap = if m.cap.eventual() {
        match m.cap.max_staleness_ms {
            Some(ms) => format!("eventual, up to {ms} ms stale"),
            None => "eventual, with no staleness budget".to_string(),
        }
    } else {
        "strong".to_string()
    };
    let plural = |n: usize, word: &str| {
        if n == 1 {
            format!("{n} {word}")
        } else {
            format!("{n} {word}s")
        }
    };
    Some(format!(
        "**{}**{}\n\ntier {} · owner {} · {cap}\n\n{} emitted · {} consumed · {}",
        m.service,
        if m.external { " (external)" } else { "" },
        m.tier.as_deref().unwrap_or("—"),
        m.owner.as_deref().unwrap_or("nobody"),
        plural(m.emits.len(), "event"),
        m.consumes.len(),
        plural(m.methods.len(), "method"),
    ))
}

/// A key says what it accepts, when that is a closed list. Anything else would
/// be repeating the word already on the screen.
fn key_hover(text: &str, line: usize, token: &str) -> Option<String> {
    let path = block_at(text, line);
    let block = resolve(schema(), &path)?;
    block.get(token)?;
    let accepted = values(&path, token, block);
    (!accepted.is_empty()).then(|| format!("**{token}** — one of `{}`", accepted.join("`, `")))
}

// ---------- go to definition ----------

/// The name under the cursor: what is inside the quotes if it is inside quotes,
/// and the word around it otherwise.
fn token_at(text: &str, line: usize, character: usize) -> Option<String> {
    let chars: Vec<char> = text.lines().nth(line)?.chars().collect();
    let quotes: Vec<usize> = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '"')
        .map(|(i, _)| i)
        .collect();
    for pair in quotes.chunks(2) {
        if let [a, b] = pair {
            if character > *a && character <= *b {
                return Some(chars[a + 1..*b].iter().collect());
            }
        }
    }
    let word = |c: &char| c.is_alphanumeric() || "_-.@".contains(*c);
    let start = chars[..character.min(chars.len())]
        .iter()
        .rposition(|c| !word(c))
        .map_or(0, |p| p + 1);
    let end = chars[start..]
        .iter()
        .position(|c| !word(c))
        .map_or(chars.len(), |p| start + p);
    let word: String = chars[start..end].iter().collect();
    // `[methods.getOrder]` is one word to an editor and two things here: the
    // block it lives in, and the name being declared. The name is the one
    // anything can be asked about.
    let name = match word.split_once('.') {
        Some((block, rest)) if schema().get(block).is_some() && !rest.is_empty() => {
            rest.to_string()
        }
        _ => word,
    };
    (!name.is_empty()).then_some(name)
}

/// Every line of every manifest that names something. The one question the
/// manifests answer and a grep does not: `getOrder` also matches `getOrderV2`,
/// and a platform where retiring a method means finding its callers cannot
/// afford that.
fn references(root: &Path, token: &str) -> Vec<Value> {
    let ms = manifest::discover(&[root.display().to_string()]).unwrap_or_default();
    let mut out = Vec::new();
    for m in &ms {
        let text = std::fs::read_to_string(&m.origin).unwrap_or_default();
        for (i, line) in text.lines().enumerate() {
            if names(line, token) {
                out.push(json!({
                    "uri": path_to_uri(&m.origin),
                    "range": {
                        "start": { "line": i as u32, "character": 0 },
                        "end": { "line": i as u32, "character": line.chars().count() as u32 }
                    }
                }));
            }
        }
    }
    out
}

/// Whether a line names a token and not one that merely starts with it.
fn names(line: &str, token: &str) -> bool {
    // the dot is a separator here and not part of a name: `[methods.getOrder]`
    // declares `getOrder`, and that line is the first answer to who names it
    let part = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || "_-@".contains(c));
    let mut rest = line;
    let mut consumed = 0;
    while let Some(at) = rest.find(token) {
        let start = consumed + at;
        let before = line[..start].chars().next_back();
        let after = line[start + token.len()..].chars().next();
        if !part(before) && !part(after) {
            return true;
        }
        consumed = start + token.len();
        rest = &line[consumed..];
    }
    false
}

/// The service named at or above a line: the one a `method = "..."` belongs
/// to. Falling through to the file's own `service` is the right answer too —
/// that is a method of this service.
fn service_above(text: &str, line: usize) -> Option<String> {
    let above: Vec<&str> = text.lines().take(line + 1).collect();
    above.iter().rev().find_map(|l| {
        let t = l.trim();
        let v = t.strip_prefix("service")?.trim_start().strip_prefix('=')?;
        Some(v.trim().trim_matches('"').to_string())
    })
}

/// Where a name is declared. An event is declared by whoever emits it, a
/// service by its own manifest, and `orders.getOrder` by the method block of
/// the service that owns it — which is the jump the manifests are full of and
/// that no editor can make on its own, because it crosses files.
fn define(root: &Path, token: &str) -> Option<Value> {
    let ms = manifest::discover(&[root.display().to_string()]).ok()?;
    if let Some(m) = ms.iter().find(|m| m.emits.contains_key(token)) {
        return Some(location(&m.origin, token));
    }
    if let Some(m) = ms.iter().find(|m| m.service == token) {
        return Some(location(&m.origin, "service"));
    }
    // `service.method`, as a dependency or a caller names it
    let (service, method) = token.split_once('.')?;
    let m = ms.iter().find(|m| m.service == service)?;
    m.methods
        .contains_key(method)
        .then(|| location(&m.origin, method))
}

/// The line a name is declared on, preferring the block header that declares
/// it over any other mention.
fn location(path: &Path, needle: &str) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let hit = text
        .lines()
        .enumerate()
        .find(|(_, l)| l.trim_start().starts_with('[') && l.contains(needle))
        .or_else(|| text.lines().enumerate().find(|(_, l)| l.contains(needle)));
    let (line, len) = hit.map_or((0, 0), |(i, l)| (i as u32, l.chars().count() as u32));
    json!({
        "uri": path_to_uri(path),
        "range": {
            "start": { "line": line, "character": 0 },
            "end": { "line": line, "character": len }
        }
    })
}

// ---------- the base protocol ----------

fn read_message(r: &mut impl BufRead) -> Option<Value> {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse().ok()?;
        }
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn send(v: &Value) {
    let body = v.to_string();
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "Content-Length: {}\r\n\r\n{body}", body.len());
    let _ = out.flush();
}

fn reply(id: Option<Value>, result: Value) {
    send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
}

fn notify(method: &str, params: Value) {
    send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
}

fn workspace_root(params: &Value) -> Option<PathBuf> {
    params
        .pointer("/workspaceFolders/0/uri")
        .or_else(|| params.get("rootUri"))
        .and_then(Value::as_str)
        .and_then(uri_to_path)
        .or_else(|| {
            params
                .get("rootPath")
                .and_then(Value::as_str)
                .map(PathBuf::from)
        })
}

fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut out = String::new();
    let mut bytes = rest.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex: String = bytes.by_ref().take(2).map(char::from).collect();
            match u8::from_str_radix(&hex, 16) {
                Ok(c) => out.push(c as char),
                Err(_) => return None,
            }
        } else {
            out.push(b as char);
        }
    }
    Some(PathBuf::from(out))
}

fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for c in path.display().to_string().chars() {
        if c.is_ascii_alphanumeric() || "/-_.~".contains(c) {
            out.push(c);
        } else {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{locate, split_owner, uri_to_path};

    /// Placing a finding is the whole feature: a diagnostic on the wrong line is
    /// worse than the terminal output it replaces.
    #[test]
    fn a_finding_lands_on_the_line_that_caused_it() {
        let text =
            "service = \"orders\"\nruntime = \"nope\"\n\n[cap]\nconsistency = \"eventual\"\n";
        // the key of a value the file does not spell the same way
        assert_eq!(
            locate(text, "orders: `runtime = \"nope\"` does not exist").0,
            1
        );
        // a block, not an assignment
        assert_eq!(locate(text, "orders: `cap` says one thing").0, 3);
        // written verbatim in the file
        assert_eq!(locate(text, "orders: `consistency` is a promise").0, 4);
        // a parse error carries its own line, counted from one
        assert_eq!(locate(text, "TOML parse error at line 2, column 11").0, 1);
        // nothing to match: the top of the file, never a panic
        assert_eq!(locate(text, "orders: no owner").0, 0);
        assert_eq!(locate("", "orders: `outbox` missing"), (0, 0));
    }

    #[test]
    fn the_owner_of_a_finding_comes_off_the_front() {
        assert_eq!(split_owner("orders: no `owner`").0, Some("orders"));
        assert_eq!(split_owner("[axon-check-x] orders: bad").0, Some("orders"));
        // a colon in a sentence is not a service
        assert_eq!(split_owner("two services emit it: orders, cart").0, None);
    }

    /// The completion is only as good as the seed: a seed that stops parsing
    /// leaves every block empty, and nothing else would say so.
    #[test]
    fn the_seed_declares_one_of_everything() {
        let s = super::schema();
        for block in [
            "cap",
            "infra",
            "api",
            "pooler",
            "analytics",
            "auth",
            "cache",
            "search",
            "patterns",
        ] {
            let keys = super::resolve(s, block).and_then(|b| b.as_object().cloned());
            assert!(
                keys.is_some_and(|k| !k.is_empty()),
                "`[{block}]` has no keys"
            );
        }
        // a map's entry, whatever it is called, has the shape of its type
        let env = super::resolve(s, "env.prod").unwrap();
        assert!(env.get("runtime").is_some(), "{env}");
        let bucket = super::resolve(s, "infra.buckets.uploads").unwrap();
        assert!(bucket.get("public").is_some(), "{bucket}");
    }

    #[test]
    fn it_offers_the_keys_of_the_block_and_the_values_of_the_key() {
        let doc = "service = \"orders\"\n\n[infra]\nstate = \"\"\n\n[patterns]\noutbox = \n";
        let labels = |line, ch| -> Vec<String> {
            super::complete(doc, line, ch)
                .iter()
                .map(|i| i["label"].as_str().unwrap().to_string())
                .collect()
        };
        // keys of the block the cursor is in
        assert!(labels(3, 0).contains(&"runtime".to_string()));
        assert!(!labels(3, 0).contains(&"consistency".to_string()));
        // the closed list of a value, with the quote the editor already typed
        let states = super::complete(doc, 3, 9);
        assert_eq!(states[0]["label"], "postgres");
        assert_eq!(states[0]["insertText"], "postgres");
        // a boolean needs no list: the model already says it is one, and it
        // goes in unquoted
        let outbox = super::complete(doc, 6, 9);
        assert_eq!(outbox[0]["insertText"], "true");
        // inside a header there is nothing to offer: the name is the author's
        assert!(super::complete(doc, 2, 3).is_empty());
    }

    /// The jump is only ever as good as what it thinks the cursor is on.
    #[test]
    fn the_name_under_the_cursor_comes_out_whole() {
        let line = "[consumes.\"order.placed@v1\"]\n";
        // inside the quotes: the event, dots and version and all
        assert_eq!(
            super::token_at(line, 0, 15).as_deref(),
            Some("order.placed@v1")
        );
        // outside them: the word around the cursor
        assert_eq!(
            super::token_at("handler = onPlaced\n", 0, 12).as_deref(),
            Some("onPlaced")
        );
        assert_eq!(super::token_at("  \n", 0, 1), None);
        // the block a name is declared in is not part of the name
        assert_eq!(
            super::token_at("[methods.getOrder]\n", 0, 12).as_deref(),
            Some("getOrder")
        );
    }

    /// Finding the callers of a method that is being retired is the point, and
    /// a prefix match would hide exactly the one that is still calling it.
    #[test]
    fn a_name_is_not_the_name_that_starts_with_it() {
        assert!(super::names("method = \"getOrder\"", "getOrder"));
        assert!(!super::names("method = \"getOrderV2\"", "getOrder"));
        assert!(super::names(
            "[emits.\"order.placed@v1\"]",
            "order.placed@v1"
        ));
        // the same line can name it twice, and the second one still counts
        assert!(super::names(
            "successor = \"getOrderV2\" # was getOrder",
            "getOrder"
        ));
    }

    #[test]
    fn an_editor_uri_comes_back_as_a_path() {
        assert_eq!(
            uri_to_path("file:///a/my%20repo/orders.toml"),
            Some("/a/my repo/orders.toml".into())
        );
        assert_eq!(uri_to_path("untitled:x"), None);
    }
}
