//! Local debugging: rebuilds what ACTUALLY happened from the causal chain.
//! No collector and no dashboard needed — the causationId is already in every
//! envelope, so an NDJSON log is enough.
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
    // The broker delivers at least once, so a real log carries the same
    // envelope more than once. A message exists once: the first appearance wins.
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Envelope>(l).ok())
        .filter(|e| seen.insert(e.id.clone()))
        .collect()
}

/// The causal tree per business flow. What gets read at 3am.
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
        out.push(format!("flow {}", root.correlation_id));
        render(root, &kids, "", true, &mut out);
    }
    if out.is_empty() {
        out.push("(no root events: the log is incomplete, or every one has a causationId)".into());
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

/// A domain event carries a version: `domain.thing@vN`. Everything else in the
/// log is an edge (an HTTP request, an RPC call): it originates the chain but is
/// not part of it.
fn is_event(t: &str) -> bool {
    t.contains('@')
}

/// The real flow as mermaid, in the same shape `axon seq --events` produces, so
/// that diffing expected against real is a text comparison.
pub fn sequence(evs: &[Envelope], only: Option<&str>) -> String {
    let by_id: HashMap<&str, &Envelope> = evs.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut out = vec!["sequenceDiagram".to_string(), "  autonumber".to_string()];
    let mut rows: Vec<&Envelope> = evs
        .iter()
        .filter(|e| only.is_none_or(|c| c == e.correlation_id))
        .collect();
    rows.sort_by(|a, b| a.time.cmp(&b.time));
    for e in rows {
        if !is_event(&e.kind) {
            continue;
        }
        // If the cause is not a domain event, the chain starts outside.
        let from = e
            .causation_id
            .as_deref()
            .and_then(|c| by_id.get(c))
            .filter(|p| is_event(&p.kind))
            .map(|p| p.source.as_str())
            .unwrap_or("client");
        out.push(format!("  {from}->>{}: {}", e.source, e.kind));
    }
    out.join("\n")
}
