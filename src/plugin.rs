//! Plugins: el modelo de `git` y `protoc`. Un plugin es un ejecutable en el
//! PATH llamado `axon-<clase>-<nombre>`. Recibe JSON por stdin y escribe por
//! stdout. Sin ABI, sin cargar librerias, sin infierno de versiones: puede
//! estar escrito en cualquier lenguaje, incluso ser un script de shell.
//!
//!   axon build --lang go        -> axon-gen-go        <- el manifiesto
//!   axon infra --target pulumi  -> axon-infra-pulumi  <- el plan neutral
//!   axon verify                 -> axon-check-*       <- todos los manifiestos
use serde::Deserialize;
use std::io::Write;
use std::process::{Command, Stdio};

/// Hallazgo que devuelve un `axon-check-*`. Un plugin de gobernanza puede
/// bloquear el pipeline igual que una regla nativa.
#[derive(Debug, Deserialize)]
pub struct Finding {
    pub level: String, // "error" | "warn"
    pub message: String,
}

pub fn run(name: &str, input: &str) -> Result<String, String> {
    let mut child = Command::new(name)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{name}: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("stdin")?
        .write_all(input.as_bytes())
        .map_err(|e| format!("{name}: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{name}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{name}: salio con {}", out.status));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("{name}: salida no es UTF-8: {e}"))
}

pub fn exists(name: &str) -> bool {
    which(name).is_some()
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let p = dir.join(name);
            p.is_file().then_some(p)
        })
    })
}

/// Todos los `axon-check-*` visibles en el PATH, ordenados y sin duplicados.
pub fn checks() -> Vec<String> {
    let mut found: Vec<String> = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .flat_map(|d| std::fs::read_dir(d).into_iter().flatten())
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with("axon-check-"))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found.dedup();
    found
}
