//! Plugins: the `git` and `protoc` model. A plugin is an executable on the
//! PATH named `axon-<kind>-<name>`. It reads JSON on stdin and writes on
//! stdout. No ABI, no library loading, no version hell: it can be written in
//! any language, and it can be a shell script.
//!
//!   axon build --lang go        -> axon-gen-go        <- the manifest
//!   axon infra --target pulumi  -> axon-infra-pulumi  <- the neutral plan
//!   axon verify                 -> axon-check-*       <- every manifest
use serde::Deserialize;
use std::io::Write;
use std::process::{Command, Stdio};

/// A finding returned by an `axon-check-*`. A governance plugin can block the
/// pipeline exactly like a native rule.
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
        return Err(format!("{name}: exited with {}", out.status));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("{name}: output is not UTF-8: {e}"))
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

/// Every `axon-check-*` visible on the PATH, sorted and deduplicated.
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
