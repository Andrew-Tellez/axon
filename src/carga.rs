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

    // Un servicio con una sola ruta autenticada necesita credencial para que
    // la medicion signifique algo.
    let protegido = routes
        .iter()
        .any(|(_, me)| me.auth.as_deref() == Some("required"));

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
        // A 429 in the overload stage is not a failure: it is the declared
        // `rate_limit` doing the one thing it exists for. Counting it as one
        // would make the ramp fail for working, and the number that matters
        // —whether it degrades or falls over— would be buried.
        let base = if con_parametro {
            "r.status === 404 || (r.status >= 200 && r.status < 300)"
        } else {
            "r.status >= 200 && r.status < 300"
        };
        let accepted = format!("{base} || r.status === 429");
        // A ramp and not a flat rate. A test that sits at the declared number
        // answers "does it hold what we said", which is worth knowing and is
        // not the interesting question: it never finds where it breaks, and it
        // never sees what happens ONE STEP past the limit — which is the moment
        // that decides whether the thing degrades or falls over.
        //
        // The stages come from `rate_limit`: half, the limit, hold there, and
        // 25% over. The last one is where the edge should be throttling with
        // 429s, because that is what declaring a limit is for.
        scenarios.push(format!(
            "    {tag}: {{\n      \
               executor: \"ramping-arrival-rate\",\n      \
               exec: \"{tag}\",\n      \
               timeUnit: \"1m\",\n      \
               startRate: {inicio},\n      \
               preAllocatedVUs: {vus},\n      maxVUs: {max},\n      \
               stages: [\n        \
                 {{ target: {mitad}, duration: `${{step}}s` }},   // half of it\n        \
                 {{ target: {rate}, duration: `${{step}}s` }},   // the declared rate_limit\n        \
                 {{ target: {rate}, duration: `${{step}}s` }},   // holding there\n        \
                 {{ target: {sobre}, duration: `${{step}}s` }},   // 25% over: does it degrade or fall over\n      \
               ],\n    }},",
            inicio = (rate / 4).max(1),
            mitad = (rate / 2).max(1),
            sobre = (rate * 5 / 4).max(rate + 1),
            vus = (rate / 6).max(2),
            // the overload stage needs headroom, or k6 throttles itself and the
            // test measures its own limit instead of the service's
            max = (rate).max(20),
        ));
        // The threshold is the declared timeout. Not a round number picked by eye.
        umbrales.push(format!(
            "    \"http_req_duration{{scenario:{tag}}}\": [\"p(95)<{timeout}\"],"
        ));
        // Every answer is either what was asked for or the declared throttle.
        // The check carries it —and not `http_req_failed`— because k6 counts a
        // 404 and a 429 as failures, and neither is one here.
        umbrales.push(format!(
            "    \"checks{{scenario:{tag}}}\": [\"rate>0.99\"],"
        ));
        // And the one that decides whether it degrades or falls over. A 429 is
        // the limit working; a 500 is the service breaking, and the ramp exists
        // to tell them apart.
        umbrales.push(format!(
            "    \"server_errors{{scenario:{tag}}}\": [\"rate<0.01\"],"
        ));
        // El cuerpo generado, y encima el que hayan dado. Los tipos del
        // manifiesto alcanzan para la FORMA y no para el contenido: `plan` es
        // un `string` y `"plan"` no es un plan, asi que sin datos de verdad
        // esto mide el camino del 422. Es el mismo verde enganoso que daba
        // medir el 401, una capa mas adentro.
        let cuerpo = if me.mutating() {
            format!(
                "JSON.stringify({{ ...{{{}}}, ...de(\"{tag}\") }})",
                me.input
                    .iter()
                    .map(|(k, t)| format!("{k}: {}", ejemplo(t, k)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            "null".into()
        };
        // La credencial viaja en TODAS las peticiones de un servicio que la
        // exige. Sin ella, k6 mide el camino del 401: la latencia sale
        // buenisima —rechazar es barato— y los umbrales de tiempo pasan
        // mientras el de checks se va a cero. Es el verde mas enganoso que
        // puede dar una prueba de carga.
        let cabeceras = match (me.mutating(), protegido) {
            (true, true) => "{ \"content-type\": \"application/json\", \"idempotency-key\": uuid(), \"authorization\": `Bearer ${token}` }",
            (true, false) => "{ \"content-type\": \"application/json\", \"idempotency-key\": uuid() }",
            (false, true) => "{ \"authorization\": `Bearer ${token}` }",
            (false, false) => "{}",
        };
        // Un GET no lleva cuerpo, asi que lo que el manifiesto declara en `in` y
        // la ruta no nombra viaja en la query. Sin esto la peticion sale sin
        // los parametros que el metodo EXIGE —`?from=&to=`— y lo que se mide
        // es como contesta el servicio a algo que nadie mandaria. Encontro un
        // 500 en un servicio real la primera vez que se genero.
        let en_ruta: Vec<&str> = route
            .split('/')
            .filter(|s| s.starts_with('{'))
            .map(|s| s.trim_matches(|c| c == '{' || c == '}'))
            .collect();
        let query = if me.mutating() {
            String::new()
        } else {
            let pares: Vec<String> = me
                .input
                .iter()
                .filter(|(k, _)| !en_ruta.contains(&k.as_str()))
                .map(|(k, t)| {
                    format!(
                        "{k}=${{encodeURIComponent(dato(\"{tag}\", \"{k}\", {}))}}",
                        ejemplo(t, k)
                    )
                })
                .collect();
            match pares.is_empty() {
                true => String::new(),
                false => format!("?{}", pares.join("&")),
            }
        };
        peticiones.push(format!(
            "export function {tag}() {{\n  \
               const r = http.request(\"{verbo}\", `${{base}}{ruta_js}{query}`, {cuerpo}, {{\n    \
                 headers: {cabeceras},\n    \
                 tags: {{ scenario: \"{tag}\" }},\n    \
                 timeout: \"{timeout}ms\",\n  }});\n  \
               check(r, {{ \"{label}\": (r) => {accepted} }}, {{ scenario: \"{tag}\" }});\n  \
               serverErrors.add(r.status >= 500, {{ scenario: \"{tag}\" }});\n  \
               throttled.add(r.status === 429, {{ scenario: \"{tag}\" }});\n}}",
            label = if con_parametro {
                "2xx, 404 —the id is made up— or 429, the declared limit"
            } else {
                "2xx or 429, the declared limit"
            },
            ruta_js = route
                .split('/')
                .map(|seg| {
                    if seg.starts_with('{') {
                        // El id de verdad si lo dieron, y uno inventado si no.
                        // Un `{tenantId}` inventado contesta 404: mide el
                        // camino, no una lectura que encontro algo.
                        format!(
                            "${{dato(\"{tag}\", \"{campo}\", uuid())}}",
                            campo = seg.trim_matches(|c| c == '{' || c == '}')
                        )
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

    // Sin credencial no se corre: medir el camino del 401 da una latencia
    // excelente y un veredicto que no significa nada. Se para al arrancar y se
    // dice como conseguirla, en vez de entregar numeros que enganan.
    let credencial = if protegido {
        "         const token = __ENV.AXON_TOKEN;\n\
         if (!token) {\n  \
           throw new Error(\n    \
             \"AXON_TOKEN no esta puesto y este servicio declara `auth = \\\"required\\\"`.\\n\" +\n    \
             \"Sin credencial esto mide el camino del 401: la latencia sale buena porque \" +\n    \
             \"rechazar es barato, y el veredicto no significa nada.\\n\" +\n    \
             \"  k6 run --env AXON_TOKEN=$(tu emisor) --env AXON_BASE=... carga.js\",\n  \
           );\n\
         }\n\n"
            .to_string()
    } else {
        String::new()
    };

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
         import {{ check }} from \"k6\";\n\
         import {{ Rate }} from \"k6/metrics\";\n\n\
         // Two of its own: a 429 is the declared limit working and a 5xx is the\n\
         // service breaking. k6 counts both as `http_req_failed`, and that is\n\
         // exactly the distinction the ramp exists to make.\n\
         const serverErrors = new Rate(\"server_errors\");\n\
         const throttled = new Rate(\"throttled\");\n\n\
         const base = __ENV.AXON_BASE || \"http://localhost:8080\";\n\
{credencial}\
         // The total, split across the four stages of the ramp. It takes `30s`\n\
         // or `30` the same: a duration written by hand should not be a trap.\n\
         const step = Math.max(\n  \
           1,\n  \
           Math.round(parseInt(String(__ENV.AXON_LOAD_DURATION || \"40s\"), 10) / 4),\n\
         );\n\
         const uuid = () =>\n  \
           \"xxxxxxxx-xxxx-4xxx-8xxx-xxxxxxxxxxxx\".replace(/x/g, () =>\n    \
             Math.floor(Math.random() * 16).toString(16),\n  );\n\n\
         // Los datos de verdad: `--env AXON_LOAD_DATA=./datos.json`, un objeto\n\
         // por metodo con lo que el manifiesto no puede saber —que un `plan`\n\
         // es `\"pro\"` y no la cadena `\"plan\"`, que ese inquilino existe—.\n\
         // Lo que traiga pisa lo generado, campo por campo, y lo que falte\n\
         // sigue siendo el ejemplo.\n\
         //\n\
         //   {{ \"registerCompany\": {{ \"plan\": \"pro\", \"taxId\": \"AAA010101AAA\" }} }}\n\
         //\n\
         // Sin esto un `POST` con el ejemplo contesta 422 y la prueba mide el\n\
         // camino del rechazo: latencia buenisima, checks en cero. Es el mismo\n\
         // verde enganoso que daba medir el 401, una capa mas adentro.\n\
         // `\"@uuid\"` como valor se cambia por uno nuevo EN CADA peticion. Es\n\
         // lo que hace falta para una llave que tiene que ser unica —un alta\n\
         // idempotente por `externalId` repite la misma fila si el valor es\n\
         // fijo, y entonces la prueba mide el camino del duplicado.\n\
         const datos = __ENV.AXON_LOAD_DATA ? JSON.parse(open(__ENV.AXON_LOAD_DATA)) : {{}};\n\
         const vivo = (v) => (v === \"@uuid\" ? uuid() : v);\n\
         const de = (tag) =>\n  \
           Object.fromEntries(Object.entries(datos[tag] ?? {{}}).map(([k, v]) => [k, vivo(v)]));\n\
         const dato = (tag, campo, porDefecto) => vivo(datos[tag]?.[campo]) ?? porDefecto;\n\n\
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

    // Una tasa de k6, por nombre de metrica. `value` y no `rate`: leer la
    // llave equivocada devuelve `None`, y `None` aqui se lee como «no hay nada
    // que decir».
    let tasa_de = |name: &str| {
        r.metrics
            .get(name)
            .and_then(|t| t.value("value").or_else(|| t.value("rate")))
    };
    for (metric, met) in &r.metrics {
        for (threshold, breached) in &met.thresholds {
            if !*breached {
                continue;
            }
            // «No aguanta la carga» es un diagnostico, y cuando NADA salio
            // bien es el diagnostico equivocado: no se midio la capacidad, se
            // midio el rechazo. Se distinguen mirando lo que el propio script
            // exporta —ni un 5xx, ni un 429 y los checks en el suelo: todo
            // fueron 4xx de peticion invalida.
            let escenario = metric
                .strip_prefix("checks{scenario:")
                .and_then(|s| s.strip_suffix('}'));
            let rechazo = escenario.is_some_and(|tag| {
                tasa_de(metric).is_some_and(|c| c < 0.5)
                    && tasa_de(&format!("server_errors{{scenario:{tag}}}")).unwrap_or(0.0) == 0.0
                    && tasa_de(&format!("throttled{{scenario:{tag}}}")).unwrap_or(0.0) == 0.0
            });
            if let (true, Some(tag)) = (rechazo, escenario) {
                errors.push(format!(
                    "{}: `{tag}` answered almost nothing with a 2xx, and not one answer was a \
                     5xx or a 429. This did not measure capacity, it measured refusal: the \
                     service rejected the requests as invalid or unauthorized. The generated \
                     body carries the manifest's SHAPE and not its content —`plan` is a \
                     `string` and `\"plan\"` is not a plan— so pass the real values with \
                     `--env AXON_LOAD_DATA=datos.json` and run it again",
                    m.service
                ));
            } else {
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

    // Did the ramp actually reach the limit? A run where nothing was throttled
    // says the declared `rate_limit` was never touched —the ramp was too short,
    // or nobody is enforcing it— and a run that only throttles says the limit
    // is below what the service is being asked for. Neither is a failure, and
    // both are worth knowing before reading the rest.
    // k6 calls a Rate's value `value` and not `rate`: reading the wrong key
    // gives `None`, and `None` here reads as "nothing to say" — a check that
    // stays quiet because it is looking in the wrong place is worse than no
    // check at all.
    let tasa = |name: &str| {
        r.metrics
            .get(name)
            .and_then(|t| t.value("value").or_else(|| t.value("rate")))
    };
    match tasa("throttled") {
        Some(0.0) => warnings.push(format!(
            "{}: not one 429 in the whole ramp. Either the declared `rate_limit` is never \
             reached, or nobody is enforcing it: the limit is then a number in a document",
            m.service
        )),
        // The ramp asks for 125% of the limit on purpose, so being throttled is
        // the expected end of it. Almost everything throttled is another
        // matter: the lowest stage —half the declared rate— was refused too.
        Some(rate) if rate > 0.8 => warnings.push(format!(
            "{}: {:.0}% of the requests were throttled, including the stage at half the \
             declared rate. The `rate_limit` is well below what is being asked of it",
            m.service,
            rate * 100.0
        )),
        _ => {}
    }
    if let Some(e) = tasa("server_errors") {
        if e > 0.0 {
            warnings.push(format!(
                "{}: {:.1}% of the answers were 5xx. Past its limit a service should degrade \
                 with 429 and not break: that is what declaring the limit was for",
                m.service,
                e * 100.0
            ));
        }
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
