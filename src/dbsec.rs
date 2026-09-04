//! Politicas de acceso a datos: RLS por fila y vistas enmascaradas por columna.
//!
//! Sale del cruce de dos cosas que axon ya sabe: el esquema real (leido de las
//! migraciones con un parser SQL) y los campos declarados PII. La salida es una
//! migracion mas, porque el esquema lo gobiernan las migraciones y no un
//! comando que se corre a mano y se olvida.
use crate::manifest::*;

/// Tablas del propio framework: no llevan inquilino ni datos personales.
const PROPIAS: [&str; 2] = ["outbox", "inbox_seen"];

/// `order` es palabra reservada, y no es la unica. Todo identificador va citado.
fn q(n: &str) -> String {
    format!("\"{}\"", n.replace('"', ""))
}

/// Regla de enmascarado para pg_anon, elegida por el TIPO de la columna.
///
/// Por el tipo y no por el nombre a proposito. Adivinar la regla porque una
/// columna se llama `email` es la misma heuristica que hace fallible al
/// `create-dict` de pg_anon, y falla en los dos sentidos: no reconoce `correo`
/// y se equivoca con `email_template`. El tipo esta declarado; el nombre no
/// significa nada.
///
/// Lo que axon garantiza es la COBERTURA: ningun campo declarado `pii` se
/// queda sin regla. La regla en si es editable —para preservar el formato de
/// un correo o un telefono— y el diccionario se versiona como cualquier otro
/// archivo.
///
/// Solo se usan `md5`, `date_trunc` y los literales, que son de Postgres, y
/// `anon_funcs.digest`, que documenta pg_anon e instala en el destino. Nada
/// inventado: una funcion que no existe hace fallar el dump a mitad de camino.
fn regla_pg_anon(c: &Column) -> String {
    let n = &c.name;
    let t = c.ty.as_str();
    if t.starts_with("uuid") {
        // md5 da 32 hex, que castea a uuid, y el formato se preserva. El
        // ::text de adentro es obligatorio: md5(uuid) no existe.
        format!("md5(\\\"{n}\\\"::text)::uuid")
    } else if t.starts_with("timestamp") || t.starts_with("date") {
        // se conserva el ano y se pierde el resto: sirve para agregados
        format!("date_trunc('year', \\\"{n}\\\")")
    } else if t.starts_with("json") {
        "'{}'::jsonb".to_string()
    } else if t.starts_with("bool") {
        "false".to_string()
    } else if t.starts_with("int")
        || t.starts_with("bigint")
        || t.starts_with("smallint")
        || t.starts_with("numeric")
        || t.starts_with("decimal")
        || t.starts_with("real")
        || t.starts_with("double")
    {
        "0".to_string()
    } else {
        format!("anon_funcs.digest(\\\"{n}\\\", 'CAMBIA_ESTE_SALT', 'sha256')")
    }
}

/// Diccionario sensible de pg_anon, derivado de lo mismo que las vistas: el
/// esquema real y los campos `pii`.
///
/// Resuelve un problema distinto al de las vistas. Las vistas protegen la
/// consulta viva: un rol de analitica nunca ve el dato crudo. pg_anon hace una
/// COPIA enmascarada, para poblar staging o darle datos realistas a alguien
/// que no debe ver los de verdad. Se complementan.
///
/// Lo que axon aporta es que el diccionario no se escribe ni se escanea: sale
/// declarado. El `create-dict` de pg_anon detecta datos sensibles por
/// heuristica, y una heuristica se equivoca en los dos sentidos.
pub fn build_pg_anon(ms: &[Manifest]) -> String {
    let esquemas = schemas(ms);
    let mut entradas = Vec::new();
    let mut excluidas = Vec::new();
    for m in ms.iter().filter(|m| !m.external) {
        let Some(tablas) = esquemas.get(&m.service) else {
            continue;
        };
        let pii: Vec<String> = m.pii.iter().map(|p| p.to_lowercase()).collect();
        for (t, cols) in tablas {
            if PROPIAS.contains(&t.as_str()) {
                // El outbox lleva el payload de cada evento en un jsonb: es el
                // ultimo lugar donde uno buscaria una fuga. Y una copia para
                // desarrollo no necesita la cola pendiente ni los ids vistos.
                excluidas.push(format!(
                    "        {{\"schema\": \"public\", \"table\": \"{t}\"}},  # infraestructura de axon"
                ));
                continue;
            }
            let sensibles: Vec<&Column> = cols
                .iter()
                .filter(|c| pii.contains(&c.name.to_lowercase()))
                .collect();
            if sensibles.is_empty() {
                continue;
            }
            let campos: Vec<String> = sensibles
                .iter()
                .map(|c| format!("                \"{}\": \"{}\",", c.name, regla_pg_anon(c)))
                .collect();
            entradas.push(format!(
                "        {{  # {}\n            \"schema\": \"public\",\n            \"table\": \"{t}\",\n            \"fields\": {{\n{}\n            }},\n        }},",
                m.service,
                campos.join("\n")
            ));
        }
    }
    let mut o = vec![
        "# generado por axon — no editar. Diccionario sensible de pg_anon:".to_string(),
        "#   axon rls manifests/ --target pg_anon > sens_dict.py".to_string(),
        "#   pg_anon --mode=dump --prepared-sens-dict-file=sens_dict.py ...".to_string(),
        "#".to_string(),
        "# Sale de los campos `pii` del manifiesto cruzados con el esquema real, asi".to_string(),
        "# que no hace falta el escaneo heuristico de `create-dict`: lo que es sensible"
            .to_string(),
        "# esta declarado, no adivinado.".to_string(),
        "#".to_string(),
        "# pg_anon hace pseudonimizacion, no anonimizacion irreversible: la copia sigue"
            .to_string(),
        "# siendo dato personal. Lo dice su propia documentacion, y conviene recordarlo"
            .to_string(),
        "# antes de mandarla a un tercero.".to_string(),
        "{".to_string(),
        "    \"dictionary\": [".to_string(),
    ];
    if entradas.is_empty() {
        o.push("        # ningun servicio declara campos `pii`".to_string());
    }
    o.extend(entradas);
    o.push("    ],".to_string());
    if !excluidas.is_empty() {
        o.push("    \"dictionary_exclude\": [".to_string());
        o.extend(excluidas);
        o.push("    ],".to_string());
    }
    o.push("}".to_string());
    o.push(String::new());
    o.join("\n")
}

pub fn build(ms: &[Manifest]) -> String {
    let esquemas = schemas(ms);
    let mut o = vec![
        "-- generado por axon — no editar. Guardalo como una migracion mas:".to_string(),
        "--   axon rls manifests/ > sql/<servicio>/090_rls.expand.sql".to_string(),
    ];
    let cabeza = o.len();
    for m in ms.iter().filter(|m| !m.external) {
        let Some(tablas) = esquemas.get(&m.service) else {
            continue;
        };
        let pii: Vec<String> = m.pii.iter().map(|p| p.to_lowercase()).collect();

        for (t, cols) in tablas {
            if PROPIAS.contains(&t.as_str()) || m.infra.tenant_exempt.contains(t) {
                continue;
            }
            // ---- RLS por fila ----
            if let Some(tenant) = &m.infra.tenant_column {
                if cols.iter().any(|c| &c.name == tenant) {
                    let (tq, cq) = (q(t), q(tenant));
                    o.push(format!(
                        "\n-- {svc}.{t}: aislamiento por inquilino\n\
                         ALTER TABLE {tq} ENABLE ROW LEVEL SECURITY;\n\
                         -- FORCE: la politica aplica tambien al dueno de la tabla, que es\n\
                         -- quien suele saltarsela sin darse cuenta.\n\
                         ALTER TABLE {tq} FORCE ROW LEVEL SECURITY;\n\
                         DROP POLICY IF EXISTS {pol} ON {tq};\n\
                         CREATE POLICY {pol} ON {tq}\n  \
                           USING ({cq} = current_setting('axon.tenant', true)::uuid)\n  \
                           WITH CHECK ({cq} = current_setting('axon.tenant', true)::uuid);",
                        svc = m.service,
                        pol = q(&format!("{t}_inquilino")),
                    ));
                }
            }
            // ---- enmascarado por columna ----
            let sensibles: Vec<&Column> = cols
                .iter()
                .filter(|c| pii.contains(&c.name.to_lowercase()))
                .collect();
            if !sensibles.is_empty() {
                let proyeccion: Vec<String> = cols
                    .iter()
                    .map(|c| {
                        if sensibles.iter().any(|s| s.name == c.name) {
                            format!("  '[redactado]'::text AS {}", q(&c.name))
                        } else {
                            format!("  {}", q(&c.name))
                        }
                    })
                    .collect();
                let (tq, vq) = (q(t), q(&format!("{t}_enmascarada")));
                o.push(format!(
                    "\n-- {svc}.{t}: vista enmascarada. Al rol de lectura se le da esta,\n\
                     -- nunca la tabla: asi un SELECT * de analitica no puede filtrar PII.\n\
                     CREATE OR REPLACE VIEW {vq} AS SELECT\n{}\nFROM {tq};\n\
                     REVOKE ALL ON {tq} FROM axon_lectura;\n\
                     GRANT SELECT ON {vq} TO axon_lectura;",
                    proyeccion.join(",\n"),
                    svc = m.service,
                ));
            }
        }
    }
    // el rol se crea una vez, no una por servicio
    if o.len() > cabeza && o.iter().any(|l| l.contains("axon_lectura")) {
        o.insert(
            cabeza,
            "\n-- El rol de lectura existe para analitica y soporte: nunca ve la tabla cruda.\n\
             DO $$ BEGIN\n  \
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'axon_lectura') THEN\n    \
                 CREATE ROLE axon_lectura NOLOGIN;\n  END IF;\nEND $$;"
                .to_string(),
        );
    }
    if o.len() <= cabeza {
        o.push(
            "\n-- Nada que generar: ningun servicio declara `tenant_column` ni campos `pii`."
                .into(),
        );
    }
    o.push(String::new());
    o.join("\n")
}
