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
    ("event_id", "string"),
    ("event_type", "string"),
    ("source", "string"),
    ("event_time", "timestamp"),
    ("trace_id", "string"),
    ("correlation_id", "string"),
    ("causation_id", "string"),
];

/// Lo que cambia entre bodegas. Nada mas que esto: el esquema y los embudos
/// son los mismos, porque salen del mismo manifiesto.
///
/// Las diferencias no son cosmeticas. BigQuery necesita `PARTITION BY`
/// explicito o cada consulta escanea el historico; Snowflake particiona solo y
/// solo acepta `CLUSTER BY`; ClickHouse necesita un motor y una clave de orden,
/// y sin `Nullable` una columna vacia guarda un cero en vez de nada.
pub struct Dialecto {
    pub nombre: &'static str,
    /// Cita un identificador.
    pub cita: fn(&str) -> String,
    /// Tipo de columna para un tipo de axon.
    pub tipo: fn(&str) -> String,
    /// Lo que va despues del parentesis de columnas.
    pub cola: fn() -> String,
    /// Diferencia en milisegundos entre dos expresiones.
    pub diff_ms: fn(&str, &str) -> String,
}

fn cita_backtick(s: &str) -> String {
    format!("`{s}`")
}
fn cita_doble(s: &str) -> String {
    format!("\"{s}\"")
}

fn tipo_bigquery(t: &str) -> String {
    match t {
        "int" => "INT64",
        "float" => "FLOAT64",
        "bool" => "BOOL",
        "timestamp" => "TIMESTAMP",
        "json" => "JSON",
        _ => "STRING",
    }
    .into()
}

fn tipo_snowflake(t: &str) -> String {
    match t {
        "int" => "NUMBER(38,0)",
        "float" => "FLOAT",
        "bool" => "BOOLEAN",
        "timestamp" => "TIMESTAMP_TZ",
        "json" => "VARIANT",
        _ => "VARCHAR",
    }
    .into()
}

/// `Nullable(DateTime64(3))` -> `DateTime64(3)`. Quitar todos los parentesis
/// del final rompe los tipos parametrizados: hay que sacar exactamente uno.
fn sin_nullable(t: &str) -> String {
    match t
        .strip_prefix("Nullable(")
        .and_then(|r| r.strip_suffix(')'))
    {
        Some(dentro) => dentro.to_string(),
        None => t.to_string(),
    }
}

fn tipo_clickhouse(t: &str) -> String {
    match t {
        "int" => "Nullable(Int64)",
        "float" => "Nullable(Float64)",
        "bool" => "Nullable(Bool)",
        // milisegundos: un evento por segundo no ordena bien un embudo
        "timestamp" => "Nullable(DateTime64(3))",
        "json" => "Nullable(String)",
        _ => "Nullable(String)",
    }
    .into()
}

pub fn dialecto(nombre: &str) -> Option<Dialecto> {
    Some(match nombre {
        "bigquery" => Dialecto {
            nombre: "bigquery",
            cita: cita_backtick,
            tipo: tipo_bigquery,
            cola: || {
                "-- particionar no es opcional: sin esto cada consulta escanea la\n\
                 -- tabla entera y la factura crece con el historico\n\
                 PARTITION BY DATE(event_time)\n\
                 CLUSTER BY correlation_id, source"
                    .into()
            },
            diff_ms: |a, b| format!("TIMESTAMP_DIFF(\n{a},\n{b},\n    MILLISECOND\n  )"),
        },
        "snowflake" => Dialecto {
            nombre: "snowflake",
            cita: cita_doble,
            tipo: tipo_snowflake,
            // Snowflake particiona solo con sus micro-particiones: declarar
            // PARTITION BY seria un error, no una optimizacion.
            cola: || "CLUSTER BY (TO_DATE(event_time), correlation_id)".into(),
            diff_ms: |a, b| format!("TIMESTAMPDIFF(\n    MILLISECOND,\n{b},\n{a}\n  )"),
        },
        "clickhouse" => Dialecto {
            nombre: "clickhouse",
            cita: cita_doble,
            tipo: tipo_clickhouse,
            cola: || {
                // El orden de las clausulas importa: ClickHouse espera ORDER BY
                // justo despues del motor. Y la clave de orden decide que
                // consultas son rapidas: primero el flujo, porque un embudo
                // agrupa por el.
                "ENGINE = MergeTree\n\
                 ORDER BY (correlation_id, event_time)\n\
                 PARTITION BY toYYYYMM(event_time)"
                    .into()
            },
            diff_ms: |a, b| format!("dateDiff(\n    'millisecond',\n{b},\n{a}\n  )"),
        },
        _ => return None,
    })
}


/// El cargador del target local: lleva el log de envelopes a ClickHouse.
///
/// El log lo escribe el propio target —`AXON_TRACE_LOG` esta en el compose
/// generado, no es un artefacto del demo— asi que la bodega local se llena de
/// la misma fuente que la traza. Y las columnas y sus rutas dentro del JSON
/// salen del mismo lugar que el esquema: si el esquema cambia, esto cambia con
/// el, que es la unica forma de que no se desincronicen.
///
/// Existe porque generar el esquema sin un camino que lo llene deja tablas
/// vacias sin un solo error, y eso es indistinguible de "no paso nada".
pub fn cargador(ms: &[Manifest], base: &str, log: &str) -> String {
    let d = dialecto("clickhouse").expect("clickhouse");
    let mut o = vec![
        "-- generado por axon — no editar.".to_string(),
        format!("--   axon analytics manifests/ --cargar {log} > cargar.sql"),
        "--".to_string(),
        "-- Idempotente por evento: se filtra por lo que ya se cargo, asi que".to_string(),
        "-- correrlo dos veces no duplica filas. Sin eso, un cargador periodico".to_string(),
        "-- multiplica cada evento por la cantidad de pasadas y el embudo miente.".to_string(),
        String::new(),
    ];
    for e in eventos(ms) {
        let t = tabla(e.nombre);
        let mut sel = vec![
            "  JSONExtractString(l, 'id')            AS event_id".to_string(),
            "  JSONExtractString(l, 'type')          AS event_type".to_string(),
            "  JSONExtractString(l, 'source')        AS source".to_string(),
            "  parseDateTime64BestEffort(JSONExtractString(l, 'time'), 3) AS event_time".to_string(),
            // el trace_id es el segundo campo del traceparent de W3C
            "  splitByChar('-', JSONExtractString(l, 'traceparent'))[2] AS trace_id".to_string(),
            "  JSONExtractString(l, 'correlationId') AS correlation_id".to_string(),
            "  nullIf(JSONExtractString(l, 'causationId'), '') AS causation_id".to_string(),
        ];
        for (campo, tipo) in e.campos {
            let sensible = es_pii(&e.pii, campo);
            if sensible && e.modo_pii == "exclude" {
                continue;
            }
            for (n, _) in columnas(&d, campo, tipo) {
                // la ruta dentro del JSON: `data.<campo>`, y `money` se aplana
                // en las dos que declara el esquema
                let ruta = if n.ends_with("_currency") {
                    format!("'data', '{campo}', 'currency'")
                } else if tipo == "money" {
                    format!("'data', '{campo}', 'amount'")
                } else {
                    format!("'data', '{campo}'")
                };
                if sensible {
                    sel.push(format!(
                        "  lower(hex(SHA256(concat({{salt:String}}, JSONExtractString(l, {ruta}))))) AS {n}_hash"
                    ));
                } else if tipo == "int" || (tipo == "money" && !n.ends_with("_currency")) {
                    sel.push(format!("  JSONExtractInt(l, {ruta}) AS {n}"));
                } else {
                    sel.push(format!("  nullIf(JSONExtractString(l, {ruta}), '') AS {n}"));
                }
            }
        }
        o.push(format!(
            "-- {} · dueno: {}\nINSERT INTO {base}.{t}\nSELECT\n{}\nFROM file('{log}', LineAsString, 'l String')\nWHERE JSONExtractString(l, 'type') = '{}'\n  -- lo ya cargado no se vuelve a cargar\n  AND JSONExtractString(l, 'id') NOT IN (SELECT event_id FROM {base}.{t});\n",
            e.nombre,
            e.duenio,
            sel.join(",\n"),
            e.nombre
        ));
    }
    o.join("\n")
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
fn columnas(d: &Dialecto, nombre: &str, tipo: &str) -> Vec<(String, String)> {
    let n = snake(nombre);
    if tipo == "money" {
        // `amount` ya dice que es un importe: `amount_amount` no aporta nada
        let importe = if n.ends_with("amount") {
            n.clone()
        } else {
            format!("{n}_amount")
        };
        vec![
            (importe, (d.tipo)("int")),
            (format!("{n}_currency"), (d.tipo)("string")),
        ]
    } else {
        vec![(n, (d.tipo)(tipo))]
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
pub fn build(ms: &[Manifest], d: &Dialecto) -> String {
    let evs = eventos(ms);
    let mut o = vec![
        "-- generado por axon — no editar.".to_string(),
        format!(
            "--   axon analytics manifests/ --target {} > bodega.sql",
            d.nombre
        ),
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
                let tipo = (d.tipo)(t);
                // en ClickHouse la nulabilidad va en el tipo, no en un sufijo
                if d.nombre == "clickhouse" {
                    let tipo = if nulo { tipo } else { sin_nullable(&tipo) };
                    format!("  {n} {tipo}")
                } else {
                    format!("  {n} {tipo}{}", if nulo { "" } else { " NOT NULL" })
                }
            })
            .collect();
        let mut excluidos = Vec::new();
        for (campo, tipo) in e.campos {
            let sensible = es_pii(&e.pii, campo);
            if sensible && e.modo_pii == "exclude" {
                excluidos.push(campo.clone());
                continue;
            }
            for (n, t) in columnas(d, campo, tipo) {
                if sensible {
                    // hash con salt, no el valor: una bodega es donde un dato
                    // personal vive mas tiempo y lo lee mas gente
                    cols.push(format!(
                        "  -- SHA-256 de `{campo}` con salt: se puede contar sin guardar\n  {n}_hash {}",
                        (d.tipo)("string")
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
            "CREATE TABLE IF NOT EXISTS {} (\n{}\n)\n{};",
            (d.cita)(&format!("@dataset.{}", tabla(e.nombre))),
            cols.join(",\n"),
            (d.cola)()
        ));
    }

    o.extend(embudos(ms, &evs, d));
    o.push(String::new());
    o.join("\n")
}

/// Vistas de embudo: una fila por flujo de negocio, con el momento de cada
/// paso y el tiempo entre ellos.
///
/// Los pasos salen de la cadena causal declarada, la misma que dibuja
/// `axon seq`. Eso es lo que hace que el embudo no sea una suposicion.
fn embudos(ms: &[Manifest], evs: &[Evento], d: &Dialecto) -> Vec<String> {
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
                    "    SELECT correlation_id, event_type, event_time FROM {}",
                    (d.cita)(&format!("@dataset.{}", tabla(e)))
                )
            })
            .collect();
        let pasos: Vec<String> = cadena
            .iter()
            .enumerate()
            .map(|(n, e)| {
                // CASE WHEN y no IF()/IFF(): es lo unico que las tres bodegas
                // entienden igual
                format!(
                    "  MIN(CASE WHEN event_type = '{e}' THEN event_time END) AS paso_{}_{}",
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
                let hasta = format!("    MIN(CASE WHEN event_type = '{e}' THEN event_time END)");
                let desde = format!(
                    "    MIN(CASE WHEN event_type = '{}' THEN event_time END)",
                    cadena[0]
                );
                format!("  {} AS ms_hasta_{}", (d.diff_ms)(&hasta, &desde), tabla(e))
            })
            .collect();

        o.push(format!(
            "\n-- Embudo de `{raiz}`: un flujo por fila.\n\
             -- Los pasos salen de la cadena causal declarada en los manifiestos, la\n\
             -- misma que dibuja `axon seq`. Un paso en NULL es un flujo que no llego\n\
             -- ahi: eso es la conversion, y el TIMESTAMP_DIFF es la latencia de\n\
             -- negocio —no la de una peticion.\n\
             CREATE OR REPLACE VIEW {} AS\n\
             SELECT\n  correlation_id,\n{},\n{}\nFROM (\n{}\n)\nGROUP BY correlation_id;",
            (d.cita)(&format!("@dataset.embudo_{}", tabla(raiz))),
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
    // el plan lleva los tipos de axon, no los de una bodega: quien lo consuma
    // los traduce a la suya
    let neutral = Dialecto {
        nombre: "plan",
        cita: |s| s.to_string(),
        tipo: |t| t.to_string(),
        cola: String::new,
        diff_ms: |a, b| format!("{a} - {b}"),
    };
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
                columnas(&neutral, n, t).into_iter().filter_map(move |(cn, ct)| {
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
