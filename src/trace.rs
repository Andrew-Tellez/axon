//! Debug local: reconstruye lo que REALMENTE paso desde la cadena causal.
//! No hace falta un colector ni un dashboard — el causationId ya esta en cada
//! envelope, asi que un log NDJSON alcanza.
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize, Clone)]
pub struct Envelope {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub source: String,
    #[serde(default)]
    pub time: String,
    #[serde(default, rename = "correlationId")]
    pub correlation_id: String,
    #[serde(default, rename = "causationId")]
    pub causation_id: Option<String>,
}

pub fn parse(text: &str) -> Vec<Envelope> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Envelope>(l).ok())
        .collect()
}

/// Arbol causal por flujo de negocio. Lo que se lee a las 3am.
pub fn tree(evs: &[Envelope], only: Option<&str>) -> String {
    let mut kids: HashMap<Option<String>, Vec<&Envelope>> = HashMap::new();
    for e in evs {
        if only.is_some_and(|c| c != e.correlation_id) {
            continue;
        }
        kids.entry(e.causation_id.clone()).or_default().push(e);
    }
    let mut out = Vec::new();
    let mut flows: Vec<&&Envelope> = kids.get(&None).into_iter().flatten().collect();
    flows.sort_by(|a, b| a.time.cmp(&b.time));
    for root in flows {
        out.push(format!("flujo {}", root.correlation_id));
        render(root, &kids, "", true, &mut out);
    }
    if out.is_empty() {
        out.push("(sin eventos raiz: el log esta incompleto o todos tienen causationId)".into());
    }
    out.join("\n")
}

fn render(
    e: &Envelope,
    kids: &HashMap<Option<String>, Vec<&Envelope>>,
    prefix: &str,
    last: bool,
    out: &mut Vec<String>,
) {
    out.push(format!(
        "{prefix}{} {} <- {}",
        if last { "└─" } else { "├─" },
        e.kind,
        e.source
    ));
    let empty = Vec::new();
    let mut next: Vec<&&Envelope> = kids
        .get(&Some(e.id.clone()))
        .unwrap_or(&empty)
        .iter()
        .collect();
    next.sort_by(|a, b| a.time.cmp(&b.time));
    let deeper = format!("{prefix}{}", if last { "   " } else { "│  " });
    for (i, k) in next.iter().enumerate() {
        render(k, kids, &deeper, i + 1 == next.len(), out);
    }
}

/// El flujo real como mermaid, para compararlo con `axon seq` (el esperado).
pub fn sequence(evs: &[Envelope], only: Option<&str>) -> String {
    let by_id: HashMap<&str, &Envelope> = evs.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut out = vec!["sequenceDiagram".to_string(), "  autonumber".to_string()];
    let mut rows: Vec<&Envelope> = evs
        .iter()
        .filter(|e| only.is_none_or(|c| c == e.correlation_id))
        .collect();
    rows.sort_by(|a, b| a.time.cmp(&b.time));
    for e in rows {
        let from = e
            .causation_id
            .as_deref()
            .and_then(|c| by_id.get(c))
            .map(|p| p.source.as_str())
            .unwrap_or("cliente");
        out.push(format!("  {from}->>{}: {}", e.source, e.kind));
    }
    out.join("\n")
}
