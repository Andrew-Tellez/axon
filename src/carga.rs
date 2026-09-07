//! Load tests derived from the manifest, and the check of what was measured.
//!
//! The manifest already declares the capacity: `rate_limit` says how much
//! traffic is expected per route, `timeout_ms` how long it may take,
//! `max_instances` how far it scales and `pool_size` how many connections each
//! instance opens. Those are all numbers, and a declared number nobody measures
//! is an opinion.
//!
//! So the script comes out of the manifest and so do the thresholds: the test
//! fails when reality does not reach what was declared. It is the same diff as
//! `axon seq` against `axon trace`, applied to performance.
use crate::manifest::*;
use serde::Deserialize;

/// The theoretical ceiling of concurrent requests the pool allows.
///
/// It is not a measurement: it is the bound what was declared imposes. If the
/// test measures more, somebody is lying; if it measures far less, the
/// bottleneck is elsewhere.
fn connection_ceiling(m: &Manifest) -> Option<u32> {
    Some(m.infra.pool_size? * m.infra.max_instances.unwrap_or(10))
}

pub fn build_k6(m: &Manifest) -> Result<String, String> {
    let routes: Vec<(&String, &Method)> = m
        .methods
        .iter()
        .filter(|(_, me)| me.http.is_some())
        .collect();
    if routes.is_empty() {
        return Err(format!("{}: exposes no HTTP routes to load", m.service));
    }

    let mut scenarios = Vec::new();
    let mut umbrales = Vec::new();
    let mut peticiones = Vec::new();
    for (name, me) in &routes {
        let verbo = me.verb().unwrap_or("GET");
        let route = me.path().unwrap_or("/");
        let tag = camel(name);
        // The declared rate is the target, not a suggestion: if the service
        // cannot take it, the manifest's rate_limit is a fiction.
        let rate = me.rate_limit.unwrap_or(60);
        let timeout = me.timeout_ms.unwrap_or(10_000);
        // A route with a parameter is tested with a made-up id, because axon
        // does not know the data. A 404 there is not a service failure: the
        // resource simply does not exist. What the test measures is the path
        // —routing, auth, a round trip to the database— not a successful read.
        let con_parametro = route.contains('{');
        let accepted = if con_parametro {
            "r.status === 404 || (r.status >= 200 && r.status < 300)"
        } else {
            "r.status >= 200 && r.status < 300"
        };
        scenarios.push(format!(
            "    {tag}: {{\n      \
               executor: \"constant-arrival-rate\",\n      \
               exec: \"{tag}\",\n      \
               rate: {rate},              // declared in rate_limit\n      \
               timeUnit: \"1m\",\n      \
               duration: __ENV.AXON_LOAD_DURATION || \"30s\",\n      \
               preAllocatedVUs: {vus},\n      maxVUs: {max},\n    }},",
            vus = (rate / 6).max(2),
            max = (rate / 2).max(10),
        ));
        // The threshold is the declared timeout. Not a round number picked by eye.
        umbrales.push(format!(
            "    \"http_req_duration{{scenario:{tag}}}\": [\"p(95)<{timeout}\"],"
        ));
        // k6 counts a 404 as a failure, so on a route with a parameter the
        // threshold goes on the check and not on the HTTP status.
        if con_parametro {
            umbrales.push(format!(
                "    \"checks{{scenario:{tag}}}\": [\"rate>0.99\"],"
            ));
        } else {
            umbrales.push(format!(
                "    \"http_req_failed{{scenario:{tag}}}\": [\"rate<0.01\"],"
            ));
        }
        let cuerpo = if me.mutating() {
            format!(
                "JSON.stringify({{{}}})",
                me.input
                    .iter()
                    .map(|(k, t)| format!("{k}: {}", ejemplo(t, k)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            "null".into()
        };
        let cabeceras = if me.mutating() {
            "{ \"content-type\": \"application/json\", \"idempotency-key\": uuid() }"
        } else {
            "{}"
        };
        peticiones.push(format!(
            "export function {tag}() {{\n  \
               const r = http.request(\"{verbo}\", `${{base}}{ruta_js}`, {cuerpo}, {{\n    \
                 headers: {cabeceras},\n    \
                 tags: {{ scenario: \"{tag}\" }},\n    \
                 timeout: \"{timeout}ms\",\n  }});\n  \
               check(r, {{ \"{label}\": (r) => {accepted} }}, {{ scenario: \"{tag}\" }});\n}}",
            label = if con_parametro {
                "2xx or 404: the id is made up"
            } else {
                "2xx"
            },
            ruta_js = route
                .split('/')
                .map(|seg| {
                    if seg.starts_with('{') {
                        "${uuid()}".to_string()
                    } else {
                        seg.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("/"),
        ));
    }

    let ceiling = connection_ceiling(m)
        .map(|c| {
            format!(
                "// Ceiling the declared pool imposes: {} connections x {} instances = {c}\n\
                 // concurrent requests. If the test plateaus before that, the bottleneck is elsewhere.\n",
                m.infra.pool_size.unwrap_or(0),
                m.infra.max_instances.unwrap_or(10)
            )
        })
        .unwrap_or_default();

    Ok(format!(
        "// generated by axon from {origin} — do not edit\n\
         //\n\
         // The thresholds are NOT numbers picked by eye: they come from the manifest.\n\
         // Each scenario runs at the rate its `rate_limit` declares and fails if the p95\n\
         // goes past its `timeout_ms`. A declared number nobody measures is an opinion.\n\
         //\n\
         //   k6 run --env AXON_BASE=http://localhost:8080 carga.js\n\
         //\n\
         {ceiling}import http from \"k6/http\";\n\
         import {{ check }} from \"k6\";\n\n\
         const base = __ENV.AXON_BASE || \"http://localhost:8080\";\n\
         const uuid = () =>\n  \
           \"xxxxxxxx-xxxx-4xxx-8xxx-xxxxxxxxxxxx\".replace(/x/g, () =>\n    \
             Math.floor(Math.random() * 16).toString(16),\n  );\n\n\
         export const options = {{\n  \
           scenarios: {{\n{scenarios}\n  }},\n  \
           thresholds: {{\n{umbrales}\n  }},\n\
         }};\n\n{peticiones}\n",
        origin = m
            .origin
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        scenarios = scenarios.join("\n"),
        umbrales = umbrales.join("\n"),
        peticiones = peticiones.join("\n\n"),
    ))
}

fn ejemplo(t: &str, k: &str) -> String {
    match t {
        "uuid" => "uuid()".into(),
        "timestamp" => "new Date().toISOString()".into(),
        "int" | "float" => "1".into(),
        "bool" => "true".into(),
        "money" => "{ amount: 100, currency: \"MXN\" }".into(),
        _ => format!("\"{k}\""),
    }
}

// ---------- the measured against the declared ----------

/// The real shape of `k6 --summary-export`: the metric's values are flat
/// junto a `thresholds`, y en ese mapa **`true` significa breached**, no ok.
/// I checked that against a real summary after assuming the opposite.
#[derive(Deserialize)]
struct Metric {
    #[serde(default)]
    thresholds: std::collections::BTreeMap<String, bool>,
    #[serde(flatten)]
    valores: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Metric {
    fn value(&self, k: &str) -> Option<f64> {
        self.valores.get(k)?.as_f64()
    }
}

#[derive(Deserialize)]
struct Summary {
    #[serde(default)]
    metrics: std::collections::BTreeMap<String, Metric>,
}

/// Reads k6's summary (`--summary-export`) and compares it with the declared.
///
/// k6 already evaluates its thresholds, but its exit code gets lost in a
/// pipeline and its report is for reading, not for diffing. This returns the
/// verdict in the shape of the rest of axon: errors with the declared number
/// next to the measured one.
pub fn review(m: &Manifest, json: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let r: Summary = serde_json::from_str(json).map_err(|e| format!("invalid summary: {e}"))?;
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());

    for (metric, met) in &r.metrics {
        for (threshold, breached) in &met.thresholds {
            if *breached {
                errors.push(format!(
                    "{}: `{metric}` breached `{threshold}`. What the manifest declares does not \
                     hold up under the traffic the manifest itself declares",
                    m.service
                ));
            }
        }
    }

    // With no thresholds in the summary there is no verdict: saying so beats
    // taking what was not measured as fine.
    if r.metrics.values().all(|m| m.thresholds.is_empty()) {
        return Err(
            "the summary carries no thresholds: run k6 with the script `axon load` generates"
                .into(),
        );
    }

    if let Some(dur) = r.metrics.get("http_req_duration") {
        if let Some(p95) = dur.value("p(95)") {
            let declared = m
                .methods
                .values()
                .filter_map(|me| me.timeout_ms)
                .max()
                .unwrap_or(10_000) as f64;
            if p95 > declared * 0.5 && p95 <= declared {
                warnings.push(format!(
                    "{}: a p95 of {p95:.0}ms against a declared timeout of {declared:.0}ms; \
                     little margin left before the timeout starts firing",
                    m.service
                ));
            }
        }
    }
    if let Some(reqs) = r.metrics.get("http_reqs") {
        if let (Some(total), Some(rate)) = (reqs.value("count"), reqs.value("rate")) {
            warnings.push(format!(
                "{}: {total:.0} requests measured at {rate:.1}/s",
                m.service
            ));
        }
        // The pool ceiling is a bound, not a measurement: if the measured
        // traffic gets close, the next bottleneck is the connections.
        if let (Some(rate), Some(ceiling)) = (reqs.value("rate"), connection_ceiling(m)) {
            if rate > f64::from(ceiling) * 0.5 {
                warnings.push(format!(
                    "{}: {rate:.1} requests/s against a ceiling of {ceiling} concurrent \
                     connections that the declared pool imposes",
                    m.service
                ));
            }
        }
    }
    Ok((errors, warnings))
}
