//! Pruebas de carga derivadas del manifiesto, y el chequeo de lo medido.
//!
//! El manifiesto ya declara la capacidad: `rate_limit` dice cuanto trafico se
//! espera por ruta, `timeout_ms` cuanto puede tardar, `max_instances` hasta
//! donde escala y `pool_size` cuantas conexiones abre cada instancia. Todo eso
//! son numeros, y un numero declarado que nadie mide es una opinion.
//!
//! Asi que el script sale del manifiesto y los umbrales tambien: la prueba
//! falla cuando la realidad no alcanza lo declarado. Es el mismo diff que
//! `axon seq` contra `axon trace`, aplicado al rendimiento.
use crate::manifest::*;
use serde::Deserialize;

/// Techo teorico de peticiones concurrentes que el pool permite.
///
/// No es una medicion: es la cota que impone lo declarado. Si la prueba mide
/// mas, alguien miente; si mide mucho menos, el cuello esta en otro lado.
fn techo_conexiones(m: &Manifest) -> Option<u32> {
    Some(m.infra.pool_size? * m.infra.max_instances.unwrap_or(10))
}

pub fn build_k6(m: &Manifest) -> Result<String, String> {
    let rutas: Vec<(&String, &Method)> = m
        .methods
        .iter()
        .filter(|(_, me)| me.http.is_some())
        .collect();
    if rutas.is_empty() {
        return Err(format!("{}: no expone rutas HTTP que cargar", m.service));
    }

    let mut escenarios = Vec::new();
    let mut umbrales = Vec::new();
    let mut peticiones = Vec::new();
    for (nombre, me) in &rutas {
        let verbo = me.verb().unwrap_or("GET");
        let ruta = me.path().unwrap_or("/");
        let tag = camel(nombre);
        // La tasa declarada es el objetivo, no una sugerencia: si el servicio
        // no la aguanta, el rate_limit del manifiesto es una ficcion.
        let rate = me.rate_limit.unwrap_or(60);
        let timeout = me.timeout_ms.unwrap_or(10_000);
        // Una ruta con parametro se prueba con un id inventado, porque axon no
        // conoce los datos. Un 404 ahi no es un fallo del servicio: es que el
        // recurso no existe. Lo que la prueba mide es el camino —ruteo, auth,
        // ida y vuelta a la base—, no una lectura exitosa.
        let con_parametro = ruta.contains('{');
        let aceptados = if con_parametro {
            "r.status === 404 || (r.status >= 200 && r.status < 300)"
        } else {
            "r.status >= 200 && r.status < 300"
        };
        escenarios.push(format!(
            "    {tag}: {{\n      \
               executor: \"constant-arrival-rate\",\n      \
               exec: \"{tag}\",\n      \
               rate: {rate},              // declarado en rate_limit\n      \
               timeUnit: \"1m\",\n      \
               duration: __ENV.AXON_CARGA_DURACION || \"30s\",\n      \
               preAllocatedVUs: {vus},\n      maxVUs: {max},\n    }},",
            vus = (rate / 6).max(2),
            max = (rate / 2).max(10),
        ));
        // El umbral es el timeout declarado. No un numero redondo elegido a ojo.
        umbrales.push(format!(
            "    \"http_req_duration{{escenario:{tag}}}\": [\"p(95)<{timeout}\"],"
        ));
        // k6 cuenta un 404 como fallo, asi que en una ruta con parametro el
        // umbral se pone sobre el check y no sobre el codigo HTTP.
        if con_parametro {
            umbrales.push(format!(
                "    \"checks{{escenario:{tag}}}\": [\"rate>0.99\"],"
            ));
        } else {
            umbrales.push(format!(
                "    \"http_req_failed{{escenario:{tag}}}\": [\"rate<0.01\"],"
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
                 tags: {{ escenario: \"{tag}\" }},\n    \
                 timeout: \"{timeout}ms\",\n  }});\n  \
               check(r, {{ \"{etiqueta}\": (r) => {aceptados} }}, {{ escenario: \"{tag}\" }});\n}}",
            etiqueta = if con_parametro {
                "2xx o 404: el id es inventado"
            } else {
                "2xx"
            },
            ruta_js = ruta
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

    let techo = techo_conexiones(m)
        .map(|c| {
            format!(
                "// Techo que impone el pool declarado: {} conexiones x {} instancias = {c}\n\
                 // peticiones concurrentes. Si la prueba se estanca antes, el cuello es otro.\n",
                m.infra.pool_size.unwrap_or(0),
                m.infra.max_instances.unwrap_or(10)
            )
        })
        .unwrap_or_default();

    Ok(format!(
        "// generado por axon desde {origen} — no editar\n\
         //\n\
         // Los umbrales NO son numeros elegidos a ojo: salen del manifiesto. Cada\n\
         // escenario corre a la tasa que declara su `rate_limit` y falla si el p95\n\
         // pasa su `timeout_ms`. Un numero declarado que nadie mide es una opinion.\n\
         //\n\
         //   k6 run --env AXON_BASE=http://localhost:8080 carga.js\n\
         //\n\
         {techo}import http from \"k6/http\";\n\
         import {{ check }} from \"k6\";\n\n\
         const base = __ENV.AXON_BASE || \"http://localhost:8080\";\n\
         const uuid = () =>\n  \
           \"xxxxxxxx-xxxx-4xxx-8xxx-xxxxxxxxxxxx\".replace(/x/g, () =>\n    \
             Math.floor(Math.random() * 16).toString(16),\n  );\n\n\
         export const options = {{\n  \
           scenarios: {{\n{escenarios}\n  }},\n  \
           thresholds: {{\n{umbrales}\n  }},\n\
         }};\n\n{peticiones}\n",
        origen = m
            .origin
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
        escenarios = escenarios.join("\n"),
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

// ---------- lo medido contra lo declarado ----------

/// Forma real de `k6 --summary-export`: los valores de la metrica van planos
/// junto a `thresholds`, y en ese mapa **`true` significa incumplido**, no ok.
/// Lo comprobe contra un resumen de verdad despues de asumir lo contrario.
#[derive(Deserialize)]
struct Metrica {
    #[serde(default)]
    thresholds: std::collections::BTreeMap<String, bool>,
    #[serde(flatten)]
    valores: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Metrica {
    fn valor(&self, k: &str) -> Option<f64> {
        self.valores.get(k)?.as_f64()
    }
}

#[derive(Deserialize)]
struct Resumen {
    #[serde(default)]
    metrics: std::collections::BTreeMap<String, Metrica>,
}

/// Lee el resumen de k6 (`--summary-export`) y lo compara con lo declarado.
///
/// k6 ya evalua sus umbrales, pero su codigo de salida se pierde en un
/// pipeline y su reporte es para leer, no para diffear. Esto devuelve el
/// veredicto en la forma del resto de axon: errores con el numero declarado al
/// lado del medido.
pub fn revisar(m: &Manifest, json: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let r: Resumen = serde_json::from_str(json).map_err(|e| format!("resumen invalido: {e}"))?;
    let (mut errores, mut avisos) = (Vec::new(), Vec::new());

    for (metrica, met) in &r.metrics {
        for (umbral, incumplido) in &met.thresholds {
            if *incumplido {
                errores.push(format!(
                    "{}: `{metrica}` incumplio `{umbral}`. Lo declarado en el manifiesto no se \
                     sostiene con el trafico que el propio manifiesto declara",
                    m.service
                ));
            }
        }
    }

    // Sin umbrales en el resumen no hay veredicto: decirlo es mejor que dar
    // por bueno lo que no se midio.
    if r.metrics.values().all(|m| m.thresholds.is_empty()) {
        return Err(
            "el resumen no trae umbrales: corre k6 con el script que genera `axon load`".into(),
        );
    }

    if let Some(dur) = r.metrics.get("http_req_duration") {
        if let Some(p95) = dur.valor("p(95)") {
            let declarado = m
                .methods
                .values()
                .filter_map(|me| me.timeout_ms)
                .max()
                .unwrap_or(10_000) as f64;
            if p95 > declarado * 0.5 && p95 <= declarado {
                avisos.push(format!(
                    "{}: p95 de {p95:.0}ms contra un timeout declarado de {declarado:.0}ms; \
                     queda poco margen antes de que el timeout empiece a dispararse",
                    m.service
                ));
            }
        }
    }
    if let Some(reqs) = r.metrics.get("http_reqs") {
        if let (Some(total), Some(rate)) = (reqs.valor("count"), reqs.valor("rate")) {
            avisos.push(format!(
                "{}: {total:.0} peticiones medidas a {rate:.1}/s",
                m.service
            ));
        }
        // El techo del pool es una cota, no una medicion: si el trafico medido
        // se acerca, el siguiente cuello de botella son las conexiones.
        if let (Some(rate), Some(techo)) = (reqs.valor("rate"), techo_conexiones(m)) {
            if rate > f64::from(techo) * 0.5 {
                avisos.push(format!(
                    "{}: {rate:.1} peticiones/s contra un techo de {techo} conexiones \
                     concurrentes que impone el pool declarado",
                    m.service
                ));
            }
        }
    }
    Ok((errores, avisos))
}
