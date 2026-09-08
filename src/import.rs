//! `axon import` — getting in without rewriting anything.
//!
//! If the team already has an event catalogue in AsyncAPI —or an HTTP API
//! documented in OpenAPI, which is what a NestJS or FastAPI repo has without
//! anybody deciding to— the manifest should not be written by hand. What the import cannot know (owner, tier, timeouts)
//! comes out as a TODO, and `axon verify` demands them: the tool leaves you in
//! an incomplete but honest state, not in one that pretends to be ready.
use serde_json::Value;

pub fn asyncapi(text: &str, service: Option<&str>) -> Result<String, String> {
    let doc: Value = parse(text)?;
    let version = doc
        .get("asyncapi")
        .and_then(Value::as_str)
        .ok_or("this does not look like an AsyncAPI document: the `asyncapi` field is missing")?;

    let name = service
        .map(str::to_string)
        .or_else(|| doc.pointer("/info/title").and_then(Value::as_str).map(slug))
        .ok_or("no `info.title`: pass the name with --service")?;

    let (emits, consumes) = match version.chars().next() {
        Some('3') => v3(&doc)?,
        Some('2') => v2(&doc)?,
        _ => return Err(format!("AsyncAPI {version} no soportado (2.x y 3.x si)")),
    };

    Ok(toml(
        &name,
        doc.pointer("/info/version").and_then(Value::as_str),
        &emits,
        &consumes,
    ))
}

/// `axon import openapi` — the format an existing HTTP service already has.
///
/// A NestJS repo has one from its decorators, a FastAPI one from its types.
/// That document already says the routes, the shapes and the statuses; what it
/// does NOT say —who owns the service, what it costs when it goes down, how
/// long a caller should wait— comes out as `TODO`, because a placeholder that
/// looks like a value is worse than an empty field.
pub fn openapi(text: &str, service: Option<&str>) -> Result<String, String> {
    let doc: Value = parse(text)?;
    let version = doc.get("openapi").and_then(Value::as_str).ok_or(
        "this does not look like an OpenAPI document: the `openapi` field is missing. \
                Swagger 2.0 carries `swagger` instead, and is not read here",
    )?;
    if !version.starts_with('3') {
        return Err(format!("OpenAPI {version} is not supported (3.x is)"));
    }
    let name = service
        .map(str::to_string)
        .or_else(|| doc.pointer("/info/title").and_then(Value::as_str).map(slug))
        .ok_or("no `info.title`: pass the name with --service")?;
    // Security declared at the top applies to every operation that does not
    // override it. Getting this backwards would mark a whole private API as
    // public, which is the error that does not look like one.
    let global_auth = doc
        .get("security")
        .and_then(Value::as_array)
        .is_some_and(|s| !s.is_empty());

    let mut methods: Vec<Method> = Vec::new();
    let paths = doc
        .get("paths")
        .and_then(Value::as_object)
        .ok_or("the document declares no `paths`")?;
    for (path, item) in paths {
        let item = deref(&doc, item);
        let common = item.get("parameters").cloned().unwrap_or(Value::Null);
        for verb in ["get", "post", "put", "patch", "delete"] {
            let Some(op) = item.get(verb) else { continue };
            let op = deref(&doc, op);
            methods.push(operation(&doc, path, verb, op, &common, global_auth));
        }
    }
    if methods.is_empty() {
        return Err("no operations found under `paths`".into());
    }
    Ok(toml_http(
        &name,
        doc.pointer("/info/version").and_then(Value::as_str),
        &methods,
    ))
}

struct Method {
    name: String,
    http: String,
    auth: &'static str,
    input: Vec<(String, String)>,
    output: Vec<(String, String)>,
    /// (code, status, detail)
    errors: Vec<(String, u64, String)>,
    deprecated: bool,
    summary: Option<String>,
}

fn operation(
    doc: &Value,
    path: &str,
    verb: &str,
    op: &Value,
    common: &Value,
    global_auth: bool,
) -> Method {
    // The name is the operationId when there is one: it is what the team
    // already calls this, and renaming it here would make the manifest talk
    // about something their code does not.
    let name = op
        .get("operationId")
        .and_then(Value::as_str)
        // `InvoicesController_findOne` is what Nest writes: the controller is
        // already named by the route, and repeating it in the method would make
        // every manifest read like a class diagram.
        .map(|s| crate::manifest::camel(s.rsplit('_').next().unwrap_or(s)))
        .unwrap_or_else(|| {
            crate::manifest::camel(&format!(
                "{verb}-{}",
                path.split('/')
                    .filter(|p| !p.is_empty() && !p.starts_with('{'))
                    .collect::<Vec<_>>()
                    .join("-")
            ))
        });

    // Path and query parameters, plus the body: for axon they are all `in`,
    // because what the method needs does not depend on how it travels.
    let mut input: Vec<(String, String)> = Vec::new();
    for list in [common, op.get("parameters").unwrap_or(&Value::Null)] {
        for prm in list.as_array().unwrap_or(&vec![]) {
            let prm = deref(doc, prm);
            let (Some(n), Some(loc)) = (
                prm.get("name").and_then(Value::as_str),
                prm.get("in").and_then(Value::as_str),
            ) else {
                continue;
            };
            // a header or a cookie is transport, not contract
            if loc != "path" && loc != "query" {
                continue;
            }
            let t = kind(doc, prm.get("schema").unwrap_or(&Value::Null));
            input.push((n.to_string(), t));
        }
    }
    input.extend(body_fields(doc, op.pointer("/requestBody")));

    let responses = op.get("responses").and_then(Value::as_object);
    let mut output = Vec::new();
    let mut errors = Vec::new();
    for (code, res) in responses.into_iter().flatten() {
        let res = deref(doc, res);
        let status: u64 = code.parse().unwrap_or(0);
        if (200..300).contains(&status) && output.is_empty() {
            output = body_fields(doc, Some(res));
        } else if (400..600).contains(&status) {
            // The status is declared; the CODE is not something OpenAPI has,
            // unless it came out of axon itself. The description is the closest
            // thing to a name a person wrote, and `verify` will demand it be
            // snake_case anyway.
            let code_name = res
                .get("x-axon-code")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    res.get("description")
                        .and_then(Value::as_str)
                        .map(|d| slug(d).replace('-', "_"))
                        .filter(|d| !d.is_empty())
                })
                .unwrap_or_else(|| format!("status_{status}"));
            errors.push((
                code_name,
                status,
                res.get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
    }

    Method {
        name,
        http: format!("{} {path}", verb.to_uppercase()),
        auth: match op.get("security").and_then(Value::as_array) {
            // an explicit empty `security: []` is how OpenAPI says "this one is
            // open", and it overrides the global one
            Some(list) if list.is_empty() => "public",
            Some(_) => "required",
            None if global_auth => "required",
            None => "public",
        },
        input,
        output,
        errors,
        deprecated: op
            .get("deprecated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        summary: op
            .get("summary")
            .or_else(|| op.get("description"))
            .and_then(Value::as_str)
            .map(|s| s.lines().next().unwrap_or(s).to_string()),
    }
}

/// The JSON fields of a request or response body. Only `application/json`:
/// anything else is not a contract axon can talk about.
fn body_fields(doc: &Value, body: Option<&Value>) -> Vec<(String, String)> {
    let Some(body) = body else { return vec![] };
    let body = deref(doc, body);
    let schema = body
        .pointer("/content/application~1json/schema")
        .map(|s| deref(doc, s));
    let Some(schema) = schema else { return vec![] };
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| {
            props
                .iter()
                .map(|(k, v)| (k.clone(), kind(doc, v)))
                .collect()
        })
        .unwrap_or_default()
}

fn fields_inline(fields: &[(String, String)]) -> String {
    fields
        .iter()
        .map(|(k, t)| format!("{k} = \"{t}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

fn toml_http(service: &str, version: Option<&str>, methods: &[Method]) -> String {
    let mut o = vec![
        "# imported from OpenAPI by axon.".to_string(),
        "# The TODOs are what the document does not say and `axon verify` will demand.".to_string(),
        String::new(),
        format!("service = \"{service}\""),
    ];
    if let Some(v) = version {
        o.push(format!("version = \"{v}\""));
    }
    o.push("owner = \"TODO\"   # the team responsible".into());
    o.push("tier  = \"TODO\"   # criticality: decides SLO and alerts".into());
    for m in methods {
        o.push(String::new());
        if let Some(s) = &m.summary {
            o.push(format!("# {s}"));
        }
        o.push(format!("[methods.{}]", m.name));
        o.push(format!("http = \"{}\"", m.http));
        o.push(format!("auth = \"{}\"", m.auth));
        // Commented out and not filled in. A budget invented here would be a
        // number nobody decided that looks decided, and `idempotent = true` is
        // worse: it says retrying does not duplicate, which is a claim about
        // code this importer has never seen. `verify` demands both where they
        // matter, and then a person answers.
        o.push("# timeout_ms = TODO   the caller's budget: with none there is no call, there is a wait".into());
        if m.http.starts_with("POST") || m.http.starts_with("PUT") || m.http.starts_with("PATCH") {
            o.push("# idempotent = TODO   only if retrying does not duplicate the effect".into());
        }
        o.push(format!("in  = {{ {} }}", fields_inline(&m.input)));
        o.push(format!("out = {{ {} }}", fields_inline(&m.output)));
        if !m.errors.is_empty() {
            o.push("# The statuses the document declares. `retriable` is not in there:".into());
            o.push("# false is the honest default, and it is what the client obeys.".into());
            o.push("errors = [".into());
            for (code, status, detail) in &m.errors {
                o.push(format!(
                    "  {{ code = \"{code}\", status = {status}{} }},",
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(", detail = \"{}\"", detail.replace('"', "'"))
                    }
                ));
            }
            o.push("]".into());
        }
        if m.deprecated {
            o.push("deprecated = \"TODO\"   # the day it stopped being the one to use".into());
            o.push("sunset     = \"TODO\"   # and the day it stops being served".into());
        }
    }
    o.push(String::new());
    o.push("[infra]".into());
    o.push("# state = \"postgres\"".into());
    o.push("# migrations = \"sql/\"".into());
    o.push(String::new());
    o.join("\n")
}

fn parse(text: &str) -> Result<Value, String> {
    if let Ok(v) = serde_json::from_str(text) {
        return Ok(v);
    }
    serde_yaml_ng::from_str(text)
        .map_err(|e| format!("this is neither valid JSON nor valid YAML: {e}"))
}

/// An event with fields and, if it came declared, its handler name.
type Event = (String, Vec<(String, String)>);

// ---------- AsyncAPI 3.x ----------

fn v3(doc: &Value) -> Result<(Vec<Event>, Vec<Event>), String> {
    let (mut emits, mut consumes) = (vec![], vec![]);
    let ops = doc.get("operations").and_then(Value::as_object);
    for (_, op) in ops.into_iter().flatten() {
        let accion = op.get("action").and_then(Value::as_str).unwrap_or("");
        let canal = op
            .pointer("/channel/$ref")
            .and_then(Value::as_str)
            .and_then(|r| resolver(doc, r));
        let Some(canal) = canal else { continue };
        let direccion = canal
            .get("address")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "no-direction".into());

        // the operation's messages, or the channel's if it does not narrow them
        let msgs: Vec<&Value> = match op.get("messages").and_then(Value::as_array) {
            Some(a) => a.iter().collect(),
            None => canal
                .get("messages")
                .and_then(Value::as_object)
                .map(|m| m.values().collect())
                .unwrap_or_default(),
        };
        for m in msgs {
            let fields = campos_de(doc, m);
            let ev = (evento(&direccion), fields);
            match accion {
                "send" => emits.push(ev),
                "receive" => consumes.push(ev),
                _ => {}
            }
        }
    }
    if emits.is_empty() && consumes.is_empty() {
        return Err("no operation with `action: send|receive` was found".into());
    }
    Ok((emits, consumes))
}

// ---------- AsyncAPI 2.x ----------

/// In 2.x the direction is from outside the app: `publish` is what others
/// publish towards it (that is, what the app consumes) and `subscribe` is what
/// the app exposes for others to read (what it emits). It is the reverse of
/// what the word suggests, and it is the number one cause of mistakes when
/// reading 2.x.
fn v2(doc: &Value) -> Result<(Vec<Event>, Vec<Event>), String> {
    let (mut emits, mut consumes) = (vec![], vec![]);
    let canales = doc
        .get("channels")
        .and_then(Value::as_object)
        .ok_or("no `channels`")?;
    for (direccion, canal) in canales {
        for (key, destino) in [("subscribe", &mut emits), ("publish", &mut consumes)] {
            let Some(op) = canal.get(key) else { continue };
            let msgs: Vec<&Value> = match op.pointer("/message/oneOf").and_then(Value::as_array) {
                Some(a) => a.iter().collect(),
                None => op.get("message").into_iter().collect(),
            };
            for m in msgs {
                destino.push((evento(direccion), campos_de(doc, m)));
            }
        }
    }
    if emits.is_empty() && consumes.is_empty() {
        return Err("the channels declare neither `publish` nor `subscribe`".into());
    }
    Ok((emits, consumes))
}

// ---------- comun ----------

fn resolver<'a>(doc: &'a Value, r: &str) -> Option<&'a Value> {
    doc.pointer(r.strip_prefix('#')?)
}

fn deref<'a>(doc: &'a Value, v: &'a Value) -> &'a Value {
    match v
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|r| resolver(doc, r))
    {
        Some(t) => t,
        None => v,
    }
}

fn campos_de(doc: &Value, msg: &Value) -> Vec<(String, String)> {
    let msg = deref(doc, msg);
    let payload = deref(doc, msg.get("payload").unwrap_or(&Value::Null));
    payload
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| {
            props
                .iter()
                .map(|(k, v)| (k.clone(), kind(doc, v)))
                .collect()
        })
        .unwrap_or_default()
}

fn kind(doc: &Value, esquema: &Value) -> String {
    let e = deref(doc, esquema);
    let t = e.get("type").and_then(Value::as_str).unwrap_or("string");
    let f = e.get("format").and_then(Value::as_str).unwrap_or("");
    match (t, f) {
        ("string", "uuid") => "uuid",
        ("string", "date-time") => "timestamp",
        ("integer", _) => "int",
        ("number", _) => "float",
        ("boolean", _) => "bool",
        // a list is not a scalar, and pretending it is one would generate a
        // column that truncates it silently
        ("array", _) => "json",
        ("object", _) => {
            // { amount, currency } is money: the dedicated type exists exactly
            // so it does not travel as a float
            let p = e.get("properties").and_then(Value::as_object);
            let tiene = |k: &str| p.is_some_and(|p| p.contains_key(k));
            if tiene("amount") && tiene("currency") {
                "money"
            } else {
                "json"
            }
        }
        _ => "string",
    }
    .into()
}

/// axon requires a version in the event's name; AsyncAPI does not carry one there.
fn evento(direccion: &str) -> String {
    if direccion.contains('@') {
        direccion.to_string()
    } else {
        format!("{direccion}@v1")
    }
}

fn slug(s: &str) -> String {
    let out: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    out.split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn handler(ev: &str) -> String {
    format!(
        "on{}",
        crate::manifest::pascal(ev.split('@').next().unwrap_or(ev))
    )
}

fn toml(service: &str, version: Option<&str>, emits: &[Event], consumes: &[Event]) -> String {
    let mut o = vec![
        "# imported from AsyncAPI by axon.".to_string(),
        "# The TODOs are what the document does not say and `axon verify` will demand.".to_string(),
        String::new(),
        format!("service = \"{service}\""),
    ];
    if let Some(v) = version {
        o.push(format!("version = \"{v}\""));
    }
    o.push("owner = \"TODO\"   # the team responsible".into());
    o.push("tier  = \"TODO\"   # criticality: decides SLO and alerts".into());

    for (ev, fields) in dedup(emits) {
        o.push(String::new());
        o.push(format!("[emits.\"{ev}\"]"));
        for (k, t) in fields {
            o.push(format!("{k} = \"{t}\""));
        }
    }
    for (ev, _) in dedup(consumes) {
        o.push(String::new());
        o.push(format!("[consumes.\"{ev}\"]"));
        o.push(format!("handler = \"{}\"", handler(&ev)));
    }
    o.push(String::new());
    o.push("[infra]".into());
    o.push("# state = \"postgres\"".into());
    o.push("# migrations = \"sql/\"".into());
    o.push(String::new());
    o.join("\n")
}

/// An event can appear in several operations; the manifest declares it once.
fn dedup(evs: &[Event]) -> Vec<Event> {
    let mut seen = std::collections::HashSet::new();
    evs.iter()
        .filter(|(ev, _)| seen.insert(ev.clone()))
        .cloned()
        .collect()
}
