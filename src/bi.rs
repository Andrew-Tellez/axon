//! Exportacion a la bodega de datos, y las metricas de negocio que salen de la
//! cadena causal declarada.
//!
//! Esto es derivable entero: axon ya conoce el esquema de cada evento, que
//! campos son personales, y —lo mas importante— **quien causa a quien**. Esa
//! ultima parte es la que ninguna bodega tiene: un embudo se arma normalmente
//! adivinando como se relacionan los eventos, y aca esta declarado.
use crate::manifest::*;
use indexmap::IndexMap;

/// Columnas del envelope, iguales en toda tabla. Son las que permiten
/// reconstruir un flujo: sin `correlation_id` no hay embudo posible.
const ENVELOPE: [(&str, &str); 7] = [
    ("event_id", "STRING"),
    ("event_type", "STRING"),
    ("source", "STRING"),
    ("event_time", "TIMESTAMP"),
    ("trace_id", "STRING"),
    ("correlation_id", "STRING"),
    ("causation_id", "STRING"),
];

fn tipo_bq(t: &str) -> &'static str {
    match t {
        "int" => "INT64",
        "float" => "FLOAT64",
        "bool" => "BOOL",
        "timestamp" => "TIMESTAMP",
        "json" => "JSON",
        _ => "STRING",
    }
}

/// Nombre de tabla a partir del evento: `order.placed@v1` -> `order_placed_v1`.
fn tabla(ev: &str) -> String {
    tfname(ev)
}

/// `customerId` -> `customer_id`. Una bodega se consulta a mano y con
/// herramientas de BI: ahi la convencion es snake_case, igual que en la base.
/// Los contratos usan la del lenguaje; la bodega, la suya.
fn snake(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                o.push('_');
            }
            o.extend(c.to_lowercase());
        } else {
            o.push(c);
        }
    }
    o
}

/// Un campo del evento a columnas. `money` se aplana en dos, que es lo que
/// hace utilizable un importe en una bodega: sumar un objeto no se puede.
fn columnas(nombre: &str, tipo: &str) -> Vec<(String, &'static str)> {
    let n = snake(nombre);
    if tipo == "money" {
        // `amount` ya dice que es un importe: `amount_amount` no aporta nada
        let importe = if n.ends_with("amount") {
            n.clone()
        } else {
            format!("{n}_amount")
        };
        vec![(importe, "INT64"), (format!("{n}_currency"), "STRING")]
    } else {
        vec![(n, tipo_bq(tipo))]
    }
}

struct Evento<'a> {
    nombre: &'a str,
    duenio: &'a str,
    campos: &'a Fields,
    pii: Vec<String>,
    modo_pii: &'a str,
}

fn eventos<'a>(ms: &'a [Manifest]) -> Vec<Evento<'a>> {
    let mut v = Vec::new();
    for m in ms.iter().filter(|m| !m.external && m.analytics.export) {
        let pii = m.pii.clone();
        for (ev, campos) in &m.emits {
            v.push(Evento {
                nombre: ev,
                duenio: &m.service,
                campos,
                pii: pii.clone(),
                modo_pii: &m.analytics.pii,
            });
        }
    }
    v
}

/// DDL de BigQuery: una tabla por evento, mas las vistas de embudo.
pub fn build_bigquery(ms: &[Manifest]) -> String {
    let evs = eventos(ms);
    let mut o = vec![
        "-- generado por axon — no editar.".to_string(),
        "--   axon analytics manifests/ --target bigquery > bodega.sql".to_string(),
        "--".to_string(),
        "-- Una tabla por evento, con las columnas del envelope que permiten".to_string(),
        "-- reconstruir un flujo, y las vistas de embudo que salen de la cadena".to_string(),
        "-- causal DECLARADA. Un embudo normalmente se arma adivinando como se".to_string(),
        "-- relacionan los eventos; aca esta escrito en el manifiesto.".to_string(),
        String::new(),
        "-- El dataset se pasa como parametro: `bq query --parameter=dataset::mi_dataset`"
            .to_string(),
        "-- o se sustituye antes de aplicar.".to_string(),
    ];
    if evs.is_empty() {
        o.push("\n-- Ningun servicio exporta eventos.".into());
        o.push(String::new());
        return o.join("\n");
    }

    for e in &evs {
        let mut cols: Vec<String> = ENVELOPE
            .iter()
            .map(|(n, t)| {
                let nulo = matches!(*n, "trace_id" | "causation_id");
                format!("  {n} {t}{}", if nulo { "" } else { " NOT NULL" })
            })
            .collect();
        let mut excluidos = Vec::new();
        for (campo, tipo) in e.campos {
            let sensible = es_pii(&e.pii, campo);
            if sensible && e.modo_pii == "exclude" {
                excluidos.push(campo.clone());
                continue;
            }
            for (n, t) in columnas(campo, tipo) {
                if sensible {
                    // hash con salt, no el valor: una bodega es donde un dato
                    // personal vive mas tiempo y lo lee mas gente
                    cols.push(format!(
                        "  -- SHA-256 de `{campo}` con salt: se puede contar sin guardar\n  {n}_hash STRING"
                    ));
                } else {
                    cols.push(format!("  {n} {t}"));
                }
            }
        }
        o.push(format!("\n-- {} · dueno: {}", e.nombre, e.duenio));
        if !excluidos.is_empty() {
            o.push(format!(
                "-- Campos personales excluidos: {}. Con `[analytics] pii = \"hash\"` se\n\
                 -- exportarian como SHA-256 con salt en vez de no exportarse.",
                excluidos.join(", ")
            ));
        }
        o.push(format!(
            "CREATE TABLE IF NOT EXISTS `@dataset.{}` (\n{}\n)\n\
             -- particionar no es opcional: sin esto cada consulta escanea la\n\
             -- tabla entera y la factura crece con el historico\n\
             PARTITION BY DATE(event_time)\n\
             CLUSTER BY correlation_id, source;",
            tabla(e.nombre),
            cols.join(",\n")
        ));
    }

    o.extend(embudos(ms, &evs));
    o.push(String::new());
    o.join("\n")
}

/// Vistas de embudo: una fila por flujo de negocio, con el momento de cada
/// paso y el tiempo entre ellos.
///
/// Los pasos salen de la cadena causal declarada, la misma que dibuja
/// `axon seq`. Eso es lo que hace que el embudo no sea una suposicion.
fn embudos(ms: &[Manifest], evs: &[Evento]) -> Vec<String> {
    let emisor: IndexMap<&str, &str> = evs.iter().map(|e| (e.nombre, e.duenio)).collect();
    let mut o = Vec::new();

    // un evento es raiz si nadie lo consume para producirlo, o sea si ningun
    // servicio lo emite como consecuencia de otro
    let derivados: Vec<&str> = ms
        .iter()
        .flat_map(|m| {
            m.emits
                .keys()
                .filter(|_| !m.consumes.is_empty())
                .map(|e| e.as_str())
        })
        .collect();
    let raices: Vec<&str> = emisor
        .keys()
        .copied()
        .filter(|e| !derivados.contains(e))
        .collect();

    for raiz in raices {
        let mut cadena = vec![raiz];
        let mut i = 0;
        while i < cadena.len() && cadena.len() < 12 {
            let actual = cadena[i];
            for m in ms {
                if m.consumes.contains_key(actual) {
                    for sig in m.emits.keys() {
                        if !cadena.contains(&sig.as_str()) && emisor.contains_key(sig.as_str()) {
                            cadena.push(sig);
                        }
                    }
                }
            }
            i += 1;
        }
        if cadena.len() < 2 {
            continue;
        }

        let union: Vec<String> = cadena
            .iter()
            .map(|e| {
                format!(
                    "    SELECT correlation_id, event_type, event_time FROM `@dataset.{}`",
                    tabla(e)
                )
            })
            .collect();
        let pasos: Vec<String> = cadena
            .iter()
            .enumerate()
            .map(|(n, e)| {
                format!(
                    "  MIN(IF(event_type = '{e}', event_time, NULL)) AS paso_{}_{}",
                    n + 1,
                    tabla(e)
                )
            })
            .collect();
        // el tiempo entre el primer paso y cada uno de los siguientes: eso es
        // la latencia del flujo de negocio, no la de una peticion
        let saltos: Vec<String> = cadena
            .iter()
            .skip(1)
            .map(|e| {
                format!(
                    "  TIMESTAMP_DIFF(\n    MIN(IF(event_type = '{e}', event_time, NULL)),\n    \
                     MIN(IF(event_type = '{}', event_time, NULL)),\n    MILLISECOND\n  ) AS ms_hasta_{}",
                    cadena[0],
                    tabla(e)
                )
            })
            .collect();

        o.push(format!(
            "\n-- Embudo de `{raiz}`: un flujo por fila.\n\
             -- Los pasos salen de la cadena causal declarada en los manifiestos, la\n\
             -- misma que dibuja `axon seq`. Un paso en NULL es un flujo que no llego\n\
             -- ahi: eso es la conversion, y el TIMESTAMP_DIFF es la latencia de\n\
             -- negocio —no la de una peticion.\n\
             CREATE OR REPLACE VIEW `@dataset.embudo_{}` AS\n\
             SELECT\n  correlation_id,\n{},\n{}\nFROM (\n{}\n)\nGROUP BY correlation_id;",
            tabla(raiz),
            pasos.join(",\n"),
            saltos.join(",\n"),
            union.join("\n    UNION ALL\n")
        ));
    }
    o
}

/// El plan neutral de la exportacion, para quien no use BigQuery.
pub fn build_plan(ms: &[Manifest]) -> serde_json::Value {
    let evs = eventos(ms);
    serde_json::json!({
        "envelope": ENVELOPE.iter().map(|(n, t)| serde_json::json!({"name": n, "type": t})).collect::<Vec<_>>(),
        "tables": evs.iter().map(|e| serde_json::json!({
            "event": e.nombre,
            "owner": e.duenio,
            "table": tabla(e.nombre),
            "partition_by": "DATE(event_time)",
            "cluster_by": ["correlation_id", "source"],
            "pii_mode": e.modo_pii,
            "columns": e.campos.iter().flat_map(|(n, t)| {
                let sensible = es_pii(&e.pii, n);
                columnas(n, t).into_iter().filter_map(move |(cn, ct)| {
                    match (sensible, e.modo_pii) {
                        (true, "exclude") => None,
                        (true, _) => Some(serde_json::json!({"name": format!("{cn}_hash"), "type": "STRING", "pii": true})),
                        _ => Some(serde_json::json!({"name": cn, "type": ct})),
                    }
                }).collect::<Vec<_>>()
            }).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}
