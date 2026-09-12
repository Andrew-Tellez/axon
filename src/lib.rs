//! The compiler, without the CLI around it.
//!
//! `axon verify` is a pure function —manifests in, findings out— and that is
//! what makes it runnable anywhere the model fits: a browser included. What
//! this exposes is that core, so the documentation can run the real compiler
//! instead of quoting an output somebody pasted and nobody re-checks.
//!
//! Only the modules the verdict needs are here. `infra`, `emit` and the rest
//! of the generators stay in the binary until something asks for them, because
//! every one of them is weight in the `.wasm`.
pub mod bi;
pub mod manifest;
pub mod verify;

#[cfg(feature = "wasm")]
mod web {
    use wasm_bindgen::prelude::*;

    /// The whole report over a workspace given as text: one TOML document per
    /// entry, named as the file would be. The names matter because the
    /// findings say who they are about.
    ///
    /// Returns the same JSON the editor gets: `{ errors: [], warnings: [] }`.
    #[wasm_bindgen]
    pub fn verify(files: &str) -> String {
        // JSON in and JSON out: the page already speaks it, and it saves a
        // crate whose whole job would be crossing the same boundary
        let files: Vec<(String, String)> = match serde_json::from_str(files) {
            Ok(f) => f,
            Err(e) => return fail(&format!("unreadable input: {e}")),
        };
        let mut ms = Vec::new();
        let mut policy = crate::verify::Policy::default();
        let mut sql = Vec::new();
        for (name, text) in &files {
            // the migrations ARE the schema, and without them half the rules
            // go quiet: a CRUD over a column nobody declared, an FK crossing a
            // service boundary
            if name.ends_with(".sql") {
                sql.push((name.clone(), text.clone()));
                continue;
            }
            // the policy is the platform's, not a service's, and it is read
            // from the same place the CLI reads it
            if name.starts_with("axon.policy") {
                if let Ok(p) = toml::from_str(text) {
                    policy = p;
                }
                continue;
            }
            if !name.ends_with(".toml") {
                continue;
            }
            match crate::manifest::parse(text, name) {
                Ok(m) => ms.push(m),
                Err(e) => return fail(&e),
            }
        }
        crate::manifest::memory::set_sql(sql);
        let r = crate::verify::verify(&ms, &policy);
        serde_json::json!({ "errors": r.errors, "warnings": r.warnings }).to_string()
    }

    fn fail(message: &str) -> String {
        serde_json::json!({ "errors": [message], "warnings": [] }).to_string()
    }
}
