//! A pact from a consumer that never adopted axon.
//!
//! The other half of the question. `axon traffic` answers who calls what,
//! because what they ask for is observable; **what they read of the answer is
//! not**, and there is no measuring that from this side. So either they tell
//! you, or they hand you something that already says it — and a consumer using
//! Pact already has that file: their pact IS the list of fields they need.
//!
//! axon does not need a broker to READ one. No credentials, no second CI job:
//! the file comes in and the compiler crosses it against what the provider
//! declares. It is the same diff as `uses`, with the input coming from outside.
use crate::manifest::{Fields, Manifest};
use indexmap::IndexMap;
use serde_json::Value;

/// What travels in a `problem+json` body, which is the same for every route.
/// axon puts the declared code in `title`.
const PROBLEM: [&str; 6] = ["type", "title", "status", "detail", "instance", "traceId"];

/// What a pact says, reduced to the two things axon can check.
pub struct Expectation {
    pub description: String,
    pub method: String,
    pub path: String,
    pub status: Option<u16>,
    /// The fields of the response body the consumer expects, flattened:
    /// `total.amount` for a nested one, because that is how the manifest names
    /// a `money` sub-field too.
    pub reads: Vec<String>,
}

pub struct Pact {
    pub consumer: String,
    pub provider: String,
    pub expectations: Vec<Expectation>,
}

fn flatten(prefix: &str, v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, inner) in map {
                let name = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                match inner {
                    Value::Object(_) => flatten(&name, inner, out),
                    // an array of objects: what matters is the shape of one
                    Value::Array(items) => {
                        if let Some(first) = items.first() {
                            flatten(&name, first, out);
                        } else {
                            out.push(name);
                        }
                    }
                    _ => out.push(name),
                }
            }
        }
        _ => {
            if !prefix.is_empty() {
                out.push(prefix.to_string());
            }
        }
    }
}

/// Reads a pact file. v2 and v3 keep the same shape for what is needed here —
/// `interactions[].request/response` — and v4 wraps each one in a `type`; the
/// ones that are not HTTP get skipped rather than guessed at.
pub fn parse(text: &str) -> Result<Pact, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("the pact is not JSON: {e}"))?;
    let name = |at: &str| -> String {
        v.get(at)
            .and_then(|c| c.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown")
            .to_string()
    };
    let mut expectations = Vec::new();
    let items = v
        .get("interactions")
        .and_then(|i| i.as_array())
        .cloned()
        .unwrap_or_default();
    for it in items {
        // v4 marks the kind; anything that is not a synchronous HTTP
        // interaction has nothing to compare against a route
        if it
            .get("type")
            .and_then(|t| t.as_str())
            .is_some_and(|t| !t.contains("HTTP") && !t.contains("Http"))
        {
            continue;
        }
        let (Some(req), Some(res)) = (it.get("request"), it.get("response")) else {
            continue;
        };
        let mut reads = Vec::new();
        if let Some(body) = res.get("body") {
            flatten("", body, &mut reads);
        }
        expectations.push(Expectation {
            description: it
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("(no description)")
                .to_string(),
            method: req
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("GET")
                .to_uppercase(),
            path: req
                .get("path")
                .and_then(|p| p.as_str())
                .unwrap_or_default()
                .to_string(),
            status: res.get("status").and_then(|s| s.as_u64()).map(|s| s as u16),
            reads,
        });
    }
    Ok(Pact {
        consumer: name("consumer"),
        provider: name("provider"),
        expectations,
    })
}

/// The declared output of the method that serves a path, flattened the same way
/// the pact's body is.
fn declared_output(fields: &Fields) -> Vec<String> {
    let mut out = Vec::new();
    for (name, kind) in fields {
        match kind.as_str() {
            // `money` is two columns and two JSON keys: the contract names the
            // field, and both sides see `{amount, currency}`
            "money" => {
                out.push(format!("{name}.amount"));
                out.push(format!("{name}.currency"));
            }
            _ => out.push(name.clone()),
        }
    }
    out
}

/// Crosses a pact against what the provider declares.
///
/// What it can answer, which is exactly what `uses` answers for a consumer
/// inside axon: whether they expect a field nobody returns —renamed, or their
/// expectation is wrong— and which declared fields no pact mentions, which is
/// the question that unfreezes a contract.
pub fn review(ms: &[Manifest], pact: &Pact) -> (Vec<String>, Vec<String>, String) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut o = Vec::new();
    let provider = ms
        .iter()
        .find(|m| !m.external && m.service == pact.provider);
    let Some(provider) = provider else {
        errors.push(format!(
            "the pact names `{}` as the provider, and there is no manifest for it. Pass the \
             provider's manifests, or the pact is about another system",
            pact.provider
        ));
        return (errors, warnings, String::new());
    };
    o.push(format!(
        "{} → {}  ·  {} interactions",
        pact.consumer,
        pact.provider,
        pact.expectations.len()
    ));

    // what the pact reads of each method, accumulated: the answer to "can I
    // remove this field" is over the whole pact and not one interaction
    let mut read_by_method: IndexMap<&str, Vec<String>> = IndexMap::new();
    for e in &pact.expectations {
        let hit = provider.methods.iter().find(|(_, me)| {
            me.verb() == Some(&e.method)
                && me
                    .path()
                    .is_some_and(|p| crate::traffic::matches(p, &e.path))
        });
        let Some((name, me)) = hit else {
            errors.push(format!(
                "`{}`: {} {} matches no declared route of {}. Either the consumer points at \
                 something that no longer exists, or it never did",
                e.description, e.method, e.path, pact.provider
            ));
            continue;
        };
        // the status: 2xx, or one the method declares
        if let Some(status) = e.status {
            let known =
                (200..300).contains(&status) || me.errors.iter().any(|f| f.status == status);
            if !known {
                warnings.push(format!(
                    "`{}`: it expects {status} from {name}, which is not 2xx nor a failure the \
                     method declares. Either the consumer is depending on an undeclared answer, \
                     or that failure should be in the manifest",
                    e.description
                ));
            }
        }
        // A failure's body is not the method's output: it is RFC 7807, the
        // same shape for every route. Comparing it against `out` would report
        // that `title` is missing from a method that never promised it, and a
        // rule with a false positive gets silenced whole.
        let is_failure = e.status.is_some_and(|s| !(200..300).contains(&s));
        if is_failure {
            for field in &e.reads {
                if !PROBLEM.contains(&field.as_str()) {
                    warnings.push(format!(
                        "`{}`: it reads `{field}` off the error body, which RFC 7807 does not \
                         define. What travels there is {}",
                        e.description,
                        PROBLEM.join(", ")
                    ));
                }
            }
        } else {
            let declared = declared_output(&me.output);
            for field in &e.reads {
                if !declared.contains(field) {
                    errors.push(format!(
                        "`{}`: it expects `{field}` in the answer of {name}, which does not \
                         return it. Either it was renamed and the consumer is reading nothing, \
                         or the pact is stale",
                        e.description
                    ));
                }
            }
            read_by_method
                .entry(name.as_str())
                .or_default()
                .extend(e.reads.clone());
        }
        o.push(format!(
            "  {} {}  →  {}.{name}  ·  reads {}{}",
            e.method,
            e.path,
            pact.provider,
            if e.reads.is_empty() {
                "nothing".to_string()
            } else {
                e.reads.join(", ")
            },
            if is_failure { "  (of the failure)" } else { "" }
        ));
    }

    // And the question that unfreezes a contract: what this consumer does NOT
    // read. It is not permission to delete —another consumer may read it— but
    // it is one name off the list of unknowns.
    for (name, reads) in &read_by_method {
        let Some(me) = provider.methods.get(*name) else {
            continue;
        };
        let unused: Vec<String> = declared_output(&me.output)
            .into_iter()
            .filter(|f| !reads.contains(f))
            .collect();
        if !unused.is_empty() {
            o.push(format!(
                "  {} does not read {} of {name}",
                pact.consumer,
                unused.join(", ")
            ));
        }
    }
    (errors, warnings, o.join("\n"))
}
