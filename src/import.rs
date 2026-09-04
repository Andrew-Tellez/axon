//! `axon import asyncapi` — entrar sin reescribir nada.
//!
//! Si el equipo ya tiene un catalogo de eventos en AsyncAPI, el manifiesto no
//! deberia escribirse a mano. Lo que el import no puede saber (dueno, tier,
//! timeouts) sale como TODO, y `axon verify` los reclama: la herramienta te
//! deja en un estado incompleto pero honesto, no en uno que finge estar listo.
use serde_json::Value;

pub fn asyncapi(text: &str, servicio: Option<&str>) -> Result<String, String> {
    let doc: Value = parse(text)?;
    let version = doc
        .get("asyncapi")
        .and_then(Value::as_str)
        .ok_or("no parece un documento AsyncAPI: falta el campo `asyncapi`")?;

    let nombre = servicio
        .map(str::to_string)
        .or_else(|| doc.pointer("/info/title").and_then(Value::as_str).map(slug))
        .ok_or("sin `info.title`: pasa el nombre con --service")?;

    let (emits, consumes) = match version.chars().next() {
        Some('3') => v3(&doc)?,
        Some('2') => v2(&doc)?,
        _ => return Err(format!("AsyncAPI {version} no soportado (2.x y 3.x si)")),
    };

    Ok(toml(
        &nombre,
        doc.pointer("/info/version").and_then(Value::as_str),
        &emits,
        &consumes,
    ))
}

fn parse(text: &str) -> Result<Value, String> {
    if let Ok(v) = serde_json::from_str(text) {
        return Ok(v);
    }
    serde_yaml_ng::from_str(text).map_err(|e| format!("no es JSON ni YAML valido: {e}"))
}

/// Un evento con campos y, si venia declarado, su nombre de handler.
type Evento = (String, Vec<(String, String)>);

// ---------- AsyncAPI 3.x ----------

fn v3(doc: &Value) -> Result<(Vec<Evento>, Vec<Evento>), String> {
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
            .unwrap_or_else(|| "sin-direccion".into());

        // los mensajes de la operacion, o los del canal si no los acota
        let msgs: Vec<&Value> = match op.get("messages").and_then(Value::as_array) {
            Some(a) => a.iter().collect(),
            None => canal
                .get("messages")
                .and_then(Value::as_object)
                .map(|m| m.values().collect())
                .unwrap_or_default(),
        };
        for m in msgs {
            let campos = campos_de(doc, m);
            let ev = (evento(&direccion), campos);
            match accion {
                "send" => emits.push(ev),
                "receive" => consumes.push(ev),
                _ => {}
            }
        }
    }
    if emits.is_empty() && consumes.is_empty() {
        return Err("no se encontro ninguna operacion con `action: send|receive`".into());
    }
    Ok((emits, consumes))
}

// ---------- AsyncAPI 2.x ----------

/// En 2.x la direccion es desde fuera de la app: `publish` es lo que otros
/// publican hacia ella (o sea, lo que la app consume) y `subscribe` lo que la
/// app expone para que otros lo lean (lo que emite). Es al reves de lo que
/// sugiere la palabra, y es la causa numero uno de errores al leer 2.x.
fn v2(doc: &Value) -> Result<(Vec<Evento>, Vec<Evento>), String> {
    let (mut emits, mut consumes) = (vec![], vec![]);
    let canales = doc
        .get("channels")
        .and_then(Value::as_object)
        .ok_or("sin `channels`")?;
    for (direccion, canal) in canales {
        for (clave, destino) in [("subscribe", &mut emits), ("publish", &mut consumes)] {
            let Some(op) = canal.get(clave) else { continue };
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
        return Err("los canales no declaran `publish` ni `subscribe`".into());
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
                .map(|(k, v)| (k.clone(), tipo(doc, v)))
                .collect()
        })
        .unwrap_or_default()
}

fn tipo(doc: &Value, esquema: &Value) -> String {
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
            // { amount, currency } es dinero: el tipo propio existe justamente
            // para que no viaje como float
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

/// axon exige version en el nombre del evento; AsyncAPI no la lleva ahi.
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

fn toml(servicio: &str, version: Option<&str>, emits: &[Evento], consumes: &[Evento]) -> String {
    let mut o = vec![
        "# importado desde AsyncAPI por axon.".to_string(),
        "# Los TODO son lo que el documento no dice y `axon verify` va a reclamar.".to_string(),
        String::new(),
        format!("service = \"{servicio}\""),
    ];
    if let Some(v) = version {
        o.push(format!("version = \"{v}\""));
    }
    o.push("owner = \"TODO\"   # equipo responsable".into());
    o.push("tier  = \"TODO\"   # criticidad: decide SLO y alertas".into());

    for (ev, campos) in dedup(emits) {
        o.push(String::new());
        o.push(format!("[emits.\"{ev}\"]"));
        for (k, t) in campos {
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

/// Un evento puede aparecer en varias operaciones; el manifiesto lo declara una vez.
fn dedup(evs: &[Evento]) -> Vec<Evento> {
    let mut vistos = std::collections::HashSet::new();
    evs.iter()
        .filter(|(ev, _)| vistos.insert(ev.clone()))
        .cloned()
        .collect()
}
