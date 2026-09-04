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
