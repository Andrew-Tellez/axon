//! `axon import asyncapi` — getting in without rewriting anything.
//!
//! If the team already has an event catalogue in AsyncAPI, the manifest should
//! not be written by hand. What the import cannot know (owner, tier, timeouts)
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

fn parse(text: &str) -> Result<Value, String> {
    if let Ok(v) = serde_json::from_str(text) {
        return Ok(v);
    }
    serde_yaml_ng::from_str(text).map_err(|e| format!("this is neither valid JSON nor valid YAML: {e}"))
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
