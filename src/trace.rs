//! Local debugging: rebuilds what ACTUALLY happened from the causal chain.
//! No collector and no dashboard needed — the causationId is already in every
//! envelope, so an NDJSON log is enough.
//!
//! And for a system that never adopted the envelope: a span IS an envelope with
//! other names. `spanId` is the id, `parentSpanId` the cause, `traceId` the
//! flow and `service.name` the source. A repo with OpenTelemetry —which is most
//! of them, without anybody deciding to— already has the real chain in its
//! trace store, and reading it needs no code change on their side.
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

/// Where the chain came from, for whoever reads the output.
pub const ENVELOPES: &str = "envelope log";
pub const OTLP: &str = "OTLP spans";
pub const JAEGER: &str = "Jaeger spans";

/// Reads whatever it is handed: axon's envelope log, OTLP JSON —what any
/// collector's file exporter writes— or Jaeger's API answer.
///
/// It is detected and not declared with a flag, because the shapes cannot be
/// confused with each other and asking twice for something the file already
/// says is one more thing to get wrong.
pub fn parse_any(text: &str) -> (Vec<Envelope>, &'static str) {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if v.get("resourceSpans").is_some() {
            return (otlp(&v), OTLP);
        }
        if v.get("data").is_some() {
            return (jaeger(&v), JAEGER);
        }
    }
    (parse(text), ENVELOPES)
}

/// OTLP JSON: `resourceSpans[].scopeSpans[].spans[]`, with the service in the
/// resource's attributes.
fn otlp(v: &serde_json::Value) -> Vec<Envelope> {
    let mut out = Vec::new();
    for rs in v["resourceSpans"].as_array().unwrap_or(&vec![]) {
        let service = rs["resource"]["attributes"]
            .as_array()
            .and_then(|attrs| {
                attrs.iter().find(|a| a["key"] == "service.name").map(|a| {
                    a["value"]["stringValue"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string()
                })
            })
            .unwrap_or_else(|| "unknown".into());
        for ss in rs["scopeSpans"].as_array().unwrap_or(&vec![]) {
            for sp in ss["spans"].as_array().unwrap_or(&vec![]) {
                let parent = sp["parentSpanId"].as_str().unwrap_or_default();
                out.push(Envelope {
                    id: sp["spanId"].as_str().unwrap_or_default().to_string(),
                    kind: sp["name"].as_str().unwrap_or_default().to_string(),
                    source: service.clone(),
                    time: sp["startTimeUnixNano"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    correlation_id: sp["traceId"].as_str().unwrap_or_default().to_string(),
                    // an empty parent is the root, not a parent called ""
                    causation_id: (!parent.is_empty()).then(|| parent.to_string()),
                });
            }
        }
    }
    out
}

/// Jaeger's API: `data[].spans[]`, with the parent in `references` and the
/// service behind a process id.
fn jaeger(v: &serde_json::Value) -> Vec<Envelope> {
    let mut out = Vec::new();
    for t in v["data"].as_array().unwrap_or(&vec![]) {
        for sp in t["spans"].as_array().unwrap_or(&vec![]) {
            let service = sp["processID"]
                .as_str()
                .and_then(|pid| t["processes"][pid]["serviceName"].as_str())
                .unwrap_or("unknown")
                .to_string();
            let parent = sp["references"]
                .as_array()
                .and_then(|refs| {
                    refs.iter()
                        .find(|r| r["refType"] == "CHILD_OF")
                        .and_then(|r| r["spanID"].as_str())
                })
                .map(str::to_string);
            out.push(Envelope {
                id: sp["spanID"].as_str().unwrap_or_default().to_string(),
                kind: sp["operationName"].as_str().unwrap_or_default().to_string(),
                source: service,
                time: sp["startTime"].to_string(),
                correlation_id: sp["traceID"].as_str().unwrap_or_default().to_string(),
                causation_id: parent,
            });
        }
    }
    out
}

/// The real edges between services, and the declared ones.
///
/// This is the half `axon traffic` cannot see: a call between services does not
/// pass through the edge. Here the parent of a span is in one service and the
/// child in another, and that IS the edge — whoever declared it or not.
pub fn edges(evs: &[Envelope], ms: &[crate::manifest::Manifest]) -> (String, bool) {
    use crate::color::{bold, green, grey, red, yellow};
    let by_id: HashMap<&str, &Envelope> = evs.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut real: Vec<(String, String)> = Vec::new();
    for e in evs {
        let Some(parent) = e.causation_id.as_deref().and_then(|c| by_id.get(c)) else {
            continue;
        };
        if parent.source != e.source && !parent.source.is_empty() && !e.source.is_empty() {
            let edge = (parent.source.clone(), e.source.clone());
            if !real.contains(&edge) {
                real.push(edge);
            }
        }
    }
    // What the manifests declare: a synchronous call, or an event somebody
    // emits and somebody else consumes. Both are an edge in the drawing, and
    // both should show up in the trace.
    let mut declared: Vec<(String, String)> = Vec::new();
    for m in ms.iter().filter(|m| !m.external) {
        for d in &m.depends {
            declared.push((m.service.clone(), d.target().to_string()));
        }
        for ev in m.emits.keys() {
            for other in ms.iter().filter(|o| o.consumes.contains_key(ev)) {
                declared.push((m.service.clone(), other.service.clone()));
            }
        }
    }
    declared.sort();
    declared.dedup();

    let known: Vec<&str> = ms.iter().map(|m| m.service.as_str()).collect();
    let mut o = vec![format!(
        "{}  {}",
        bold(&format!("{} edges between services", real.len())),
        grey(&format!("{} declared", declared.len()))
    )];
    let mut fails = false;
    for (from, to) in &real {
        // An edge that nobody declared is a dependency that EXISTS. It is not a
        // style opinion: it is the drawing being wrong, and the drawing is what
        // somebody reads before deciding what can be deployed apart.
        let is_declared = declared.iter().any(|(a, b)| a == from && b == to);
        let both_known = known.contains(&from.as_str()) && known.contains(&to.as_str());
        if !is_declared && both_known {
            fails = true;
            o.push(format!(
                "  {} {from} → {to}  {}",
                red("undeclared"),
                grey("nobody declares this dependency, and it happened")
            ));
        } else if !is_declared {
            o.push(format!(
                "  {} {from} → {to}  {}",
                yellow("outside"),
                grey("one of the two has no manifest here")
            ));
        } else {
            o.push(format!("  {} {from} → {to}", green("ok")));
        }
    }
    for (from, to) in &declared {
        if !real.iter().any(|(a, b)| a == from && b == to) {
            o.push(format!(
                "  {} {from} → {to}  {}",
                grey("quiet"),
                grey("declared and not seen in this trace")
            ));
        }
    }
    if fails {
        o.push(format!(
            "\n{} a dependency that happens and nobody declares is the drawing being wrong, \
             and the drawing is what somebody reads before deciding what can be deployed apart",
            red("fail")
        ));
    }
    (format!("{}\n", o.join("\n")), fails)
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
