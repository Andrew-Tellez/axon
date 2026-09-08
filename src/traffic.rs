//! Who calls what, read from the edge.
//!
//! A caller outside axon declares nothing, and there is an asymmetry that
//! decides what can be known about it: **what it asks for is observable and
//! what it reads of the answer is not**. Four fields go out; which ones the
//! other side looked at never comes back.
//!
//! So this answers the half that has an answer, and it is the half that hurts
//! most when a version is being retired: who is still calling `/v1`, how much,
//! and from where. The other half —which fields they read— has to be told or
//! handed over, and `axon pact` is that door.
//!
//! Same shape as everything else that touches a live system: axon does not
//! connect to the edge. The log is a file, it comes in through `--check`, and
//! the compiler crosses it against what the manifests declare.
use crate::manifest::{date, epoch_days, today, Manifest};
use indexmap::IndexMap;

/// One request, as much of it as any edge bothers to write down.
pub struct Hit {
    pub method: String,
    pub path: String,
    pub client: String,
    pub version: Option<String>,
}

/// Reads the access log. NDJSON, tolerant about the names: Traefik writes
/// `RequestMethod`, Envoy and most others write lowercase, and an ALB writes
/// something else again. Being strict here would mean the tool only works with
/// the edge axon itself generates, which is the opposite of the point.
pub fn parse(text: &str) -> Vec<Hit> {
    let field = |v: &serde_json::Value, names: &[&str]| -> Option<String> {
        for n in names {
            match v.get(n) {
                Some(serde_json::Value::String(s)) if !s.is_empty() => return Some(s.clone()),
                _ => {}
            }
        }
        None
    };
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let (Some(method), Some(path)) = (
            field(&v, &["RequestMethod", "method", "request_method", "verb"]),
            field(
                &v,
                &["RequestPath", "path", "request_path", "uri", "request_uri"],
            ),
        ) else {
            continue;
        };
        out.push(Hit {
            method: method.to_uppercase(),
            // the query string is not part of the route
            path: path.split('?').next().unwrap_or(&path).to_string(),
            client: field(
                &v,
                &["ClientHost", "client", "remote_addr", "client_ip", "source"],
            )
            .unwrap_or_else(|| "unknown".into()),
            version: field(
                &v,
                &[
                    "request_X-Api-Version",
                    "x-api-version",
                    "X-Api-Version",
                    "api_version",
                ],
            ),
        });
    }
    out
}

/// Whether a concrete path is an instance of a declared route. `{tenantId}`
/// takes one segment and no more: without that, `/v1/orders/{id}` would swallow
/// `/v1/orders/{id}/refunds` and the traffic of one route would be counted as
/// the other's.
pub fn matches(template: &str, path: &str) -> bool {
    let a: Vec<&str> = template.split('/').collect();
    let b: Vec<&str> = path.split('/').collect();
    a.len() == b.len()
        && a.iter()
            .zip(&b)
            .all(|(t, p)| (t.starts_with('{') && t.ends_with('}') && !p.is_empty()) || t == p)
}

struct Row {
    service: String,
    method: String,
    route: String,
    calls: usize,
    clients: usize,
    deprecated: Option<String>,
    sunset: Option<String>,
    successor: Option<String>,
}

/// The report, and whether it has to fail.
///
/// It fails on one fact and not on a judgement: traffic on a route whose
/// declared sunset has ALREADY passed. That is not "somebody should migrate",
/// it is a retirement that was announced and did not happen, and the log is the
/// only place it shows.
pub fn report(ms: &[Manifest], hits: &[Hit]) -> (String, bool) {
    use crate::color::{bold, green, grey, red, yellow};
    let now = today();
    let mut rows: Vec<Row> = Vec::new();
    for m in ms.iter().filter(|m| !m.external) {
        for (name, me) in &m.methods {
            let (Some(verb), Some(route)) = (me.verb(), me.path()) else {
                continue;
            };
            let mine: Vec<&Hit> = hits
                .iter()
                .filter(|h| h.method == verb && matches(route, &h.path))
                .collect();
            let mut clients: IndexMap<&str, ()> = IndexMap::new();
            for h in &mine {
                clients.insert(h.client.as_str(), ());
            }
            rows.push(Row {
                service: m.service.clone(),
                method: name.clone(),
                route: format!("{verb} {route}"),
                calls: mine.len(),
                clients: clients.len(),
                deprecated: me.deprecated.clone(),
                sunset: me.sunset.clone(),
                successor: me.successor.clone(),
            });
        }
    }
    // whatever matched no declared route: a client pointing at something
    // nobody declares, or a route somebody serves outside the manifest
    let mut strays: IndexMap<String, usize> = IndexMap::new();
    for h in hits {
        let known = rows.iter().any(|r| {
            r.route
                .split_once(' ')
                .is_some_and(|(v, t)| v == h.method && matches(t, &h.path))
        });
        if !known {
            *strays
                .entry(format!("{} {}", h.method, h.path))
                .or_insert(0) += 1;
        }
    }

    let mut o = vec![format!(
        "{}  {}",
        bold(&format!("{} requests", hits.len())),
        grey(&format!("{} declared routes", rows.len()))
    )];
    let mut fails = false;
    rows.sort_by_key(|r| std::cmp::Reverse(r.calls));
    for r in &rows {
        let retiring = match (&r.deprecated, &r.sunset) {
            (None, None) => String::new(),
            (_, sunset) => {
                let left = sunset.as_deref().and_then(date).map(|s| {
                    let d = epoch_days(s) - epoch_days(now);
                    if d < 0 {
                        // announced and not done: the traffic proves it
                        if r.calls > 0 {
                            fails = true;
                        }
                        "PAST ITS SUNSET".to_string()
                    } else {
                        format!("{d}d left")
                    }
                });
                format!(
                    " · deprecated{}{}",
                    left.map(|l| format!(" · {l}")).unwrap_or_default(),
                    r.successor
                        .as_deref()
                        .map(|s| format!(" · use `{s}`"))
                        .unwrap_or_default()
                )
            }
        };
        let paint = if r.calls == 0 {
            grey
        } else if fails && r.deprecated.is_some() {
            red
        } else if r.deprecated.is_some() {
            yellow
        } else {
            green
        };
        o.push(format!(
            "  {:>7}  {:>3}  {}{}",
            paint(&r.calls.to_string()),
            grey(&r.clients.to_string()),
            r.route,
            grey(&retiring)
        ));
    }
    // A route with no traffic is the other half of the same question — it may
    // be code kept alive for nobody — but the edge only sees what comes from
    // outside. A service calling another does not pass through here, so this
    // says "no traffic through the edge" and not "nobody calls it": the
    // difference is a route somebody would delete off a wrong reading.
    let quiet: Vec<&Row> = rows.iter().filter(|r| r.calls == 0).collect();
    if !quiet.is_empty() && !hits.is_empty() {
        o.push(format!(
            "\n{}  {}\n  {}",
            grey("no traffic through the edge"),
            grey(
                &quiet
                    .iter()
                    .map(|r| format!("{}.{}", r.service, r.method))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            grey("a call between services does not pass through the edge; this is not proof that nobody calls it")
        ));
    }
    if !strays.is_empty() {
        o.push(format!(
            "\n{}  {} requests matched no declared route:",
            yellow("undeclared"),
            strays.values().sum::<usize>()
        ));
        for (route, n) in strays.iter().take(8) {
            o.push(format!("  {:>7}  {route}", n));
        }
    }
    // And with the dated scheme, the question is not which route but which
    // VERSION each caller pinned. It is the same asymmetry: what they pin is
    // observable, what they read is not.
    if let Some(api) = ms.iter().find(|m| !m.external).map(|m| &m.api) {
        if api.by_header() {
            let mut per: IndexMap<String, usize> = IndexMap::new();
            for h in hits {
                *per.entry(
                    h.version
                        .clone()
                        .unwrap_or_else(|| "(pinned nothing)".into()),
                )
                .or_insert(0) += 1;
            }
            o.push(format!("\n{}", bold("pinned versions")));
            for (v, n) in &per {
                let state = api
                    .find(v)
                    .map(|ver| {
                        let stage = ver.stage(now, api.current().unwrap_or_default());
                        if stage == "retired" && *n > 0 {
                            fails = true;
                        }
                        stage.to_string()
                    })
                    .unwrap_or_else(|| {
                        if v.starts_with('(') {
                            format!("gets {}", api.current().unwrap_or_default())
                        } else {
                            "NOT DECLARED".to_string()
                        }
                    });
                o.push(format!("  {:>7}  {v}  {}", n, grey(&state)));
            }
        }
    }
    if fails {
        o.push(format!(
            "\n{} something past its declared sunset is still being called. It was \
             announced and it did not happen, and this log is the only place it shows",
            red("fail")
        ));
    }
    (format!("{}\n", o.join("\n")), fails)
}
