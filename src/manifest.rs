//! Carga, descubrimiento y esquema derivado de migraciones.
use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub type Fields = IndexMap<String, String>;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Consume {
    pub handler: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Method {
    #[serde(rename = "in", default)]
    pub input: Fields,
    #[serde(rename = "out", default)]
    pub output: Fields,
    /// Exposicion HTTP: "POST /payments". Sin esto el metodo es solo RPC interno.
    pub http: Option<String>,
    /// Reintentable sin efectos duplicados. Obligatorio en metodos mutantes.
    #[serde(default)]
    pub idempotent: bool,
    /// Devuelve coleccion: obliga paginacion por cursor.
    #[serde(default)]
    pub paginated: bool,
}

impl Method {
    pub fn verb(&self) -> Option<&str> {
        self.http.as_ref()?.split_whitespace().next()
    }
    pub fn path(&self) -> Option<&str> {
        self.http.as_ref()?.split_whitespace().nth(1)
    }
    pub fn mutating(&self) -> bool {
        matches!(self.verb(), Some("POST" | "PUT" | "PATCH" | "DELETE"))
    }
    /// GET/HEAD lo son por definicion; lo demas hay que declararlo.
    pub fn is_idempotent(&self) -> bool {
        self.idempotent || !self.mutating()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Depend {
    pub service: Option<String>,
    pub external: Option<String>,
    pub method: String,
    /// Handler concreto que hace la llamada; afina el diagrama de secuencia.
    pub via: Option<String>,
    /// Toda llamada de red tiene presupuesto de tiempo. Obligatorio.
    pub timeout_ms: Option<u32>,
    #[serde(default)]
    pub retries: u32,
    /// Corta la cascada cuando el otro lado se cae.
    #[serde(default)]
    pub breaker: bool,
}

impl Depend {
    pub fn target(&self) -> &str {
        self.service
            .as_deref()
            .or(self.external.as_deref())
            .unwrap_or("?")
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Patterns {
    #[serde(default)]
    pub outbox: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Infra {
    pub state: Option<String>,
    pub runtime: Option<String>,
    /// Directorio de migraciones: la fuente de verdad del esquema.
    pub migrations: Option<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    pub min_instances: Option<u32>,
    pub max_instances: Option<u32>,
    /// Puerto HTTP del contenedor.
    pub port: Option<u16>,
}

/// Una transicion. El QUE es portable a cualquier lenguaje; el COMO
/// (el cuerpo del handler) lo escribe la persona, siempre.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Transition {
    pub from: Vec<String>,
    pub to: String,
    /// Metodo o evento que la dispara.
    pub on: String,
    /// Evento que se emite al completarla.
    pub emits: Option<String>,
    /// Transicion inversa, para sagas: que deshace este paso.
    pub compensates: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Machine {
    pub initial: String,
    #[serde(default, rename = "final")]
    pub final_states: Vec<String>,
    #[serde(default)]
    pub transitions: IndexMap<String, Transition>,
}

impl Machine {
    pub fn states(&self) -> Vec<String> {
        let mut s = vec![self.initial.clone()];
        for t in self.transitions.values() {
            for f in &t.from {
                if !s.contains(f) {
                    s.push(f.clone());
                }
            }
            if !s.contains(&t.to) {
                s.push(t.to.clone());
            }
        }
        for f in &self.final_states {
            if !s.contains(f) {
                s.push(f.clone());
            }
        }
        s
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Manifest {
    pub service: String,
    pub version: Option<String>,
    /// Gobernanza: todo servicio tiene dueno humano y criticidad declarada.
    pub owner: Option<String>,
    pub tier: Option<String>,
    #[serde(default)]
    pub external: bool,
    #[serde(default)]
    pub emits: IndexMap<String, Fields>,
    #[serde(default)]
    pub consumes: IndexMap<String, Consume>,
    #[serde(default)]
    pub methods: IndexMap<String, Method>,
    #[serde(default)]
    pub depends: Vec<Depend>,
    #[serde(default)]
    pub patterns: Patterns,
    /// Maquinas de estado del dominio: la unica logica de negocio que vale
    /// la pena declarar, porque es la misma en todos los lenguajes.
    #[serde(default)]
    pub machine: IndexMap<String, Machine>,
    #[serde(default)]
    pub infra: Infra,
    /// Overrides por entorno: `[env.prod] min_instances = 3`.
    #[serde(default)]
    pub env: IndexMap<String, Infra>,
    #[serde(skip)]
    pub origin: PathBuf,
}

/// Aplica los overrides de un entorno sobre la infra base. El manifiesto
/// sigue siendo uno solo: los entornos son deltas, no copias.
pub fn for_env(m: &Manifest, env: &str) -> Manifest {
    let mut out = m.clone();
    if let Some(o) = m.env.get(env) {
        if o.state.is_some() {
            out.infra.state = o.state.clone();
        }
        if o.runtime.is_some() {
            out.infra.runtime = o.runtime.clone();
        }
        if o.migrations.is_some() {
            out.infra.migrations = o.migrations.clone();
        }
        if o.min_instances.is_some() {
            out.infra.min_instances = o.min_instances;
        }
        if !o.secrets.is_empty() {
            out.infra.secrets = o.secrets.clone();
        }
    }
    out
}

pub fn load(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut m: Manifest = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    m.origin = path.to_path_buf();
    Ok(m)
}

/// Fusiona manifiestos de disco y de servicios vivos. Un servicio publica el
/// suyo en /.well-known/axon.json; uno externo se congela en un *.external.toml.
pub fn discover(sources: &[String]) -> Result<Vec<Manifest>, String> {
    let mut out = Vec::new();
    for s in sources {
        if s.starts_with("http://") || s.starts_with("https://") {
            let url = if s.ends_with(".json") {
                s.clone()
            } else {
                format!("{}/.well-known/axon.json", s.trim_end_matches('/'))
            };
            match ureq::get(&url)
                .timeout(std::time::Duration::from_secs(5))
                .call()
            {
                Ok(r) => {
                    let mut m: Manifest = r.into_json().map_err(|e| format!("{url}: {e}"))?;
                    m.origin = PathBuf::from(&url);
                    out.push(m);
                }
                // un servicio caido no rompe el descubrimiento
                Err(e) => eprintln!("axon: {url}: {e}"),
            }
        } else {
            let p = Path::new(s);
            if p.is_dir() {
                let mut files: Vec<_> = std::fs::read_dir(p)
                    .map_err(|e| format!("{s}: {e}"))?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                    // axon.*.toml es configuracion del propio axon, no un servicio
                    .filter(|p| {
                        !p.file_name()
                            .is_some_and(|n| n.to_string_lossy().starts_with("axon."))
                    })
                    .collect();
                files.sort();
                for f in files {
                    out.push(load(&f)?);
                }
            } else {
                out.push(load(p)?);
            }
        }
    }
    Ok(out)
}

// ---------- esquema derivado de las migraciones ----------

#[derive(Debug, Clone, Serialize)]
pub struct Column {
    pub name: String,
    pub ty: String,
    pub pk: bool,
    pub fk: Option<String>,
}

pub type Tables = IndexMap<String, Vec<Column>>;

// ponytail: regex, no parser SQL. Aguanta CREATE TABLE / ALTER TABLE normales.
// Si se rompe: `pg_dump --schema-only` es mas regular, e information_schema
// es la salida definitiva (cuesta una dependencia de driver).
static CREATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?is)create\s+table\s+(?:if\s+not\s+exists\s+)?"?([\w.]+)"?\s*\((.*?)\n\s*\)\s*;"#,
    )
    .unwrap()
});
static ADD_COL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)alter\s+table\s+"?([\w.]+)"?\s+add\s+column\s+(?:if\s+not\s+exists\s+)?"?(\w+)"?\s+(\w+)"#).unwrap()
});
static DROP_COL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?is)alter\s+table\s+"?([\w.]+)"?\s+drop\s+column\s+(?:if\s+exists\s+)?"?(\w+)"?"#,
    )
    .unwrap()
});
static DROP_TABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)drop\s+table\s+(?:if\s+exists\s+)?"?([\w.]+)"?"#).unwrap());
static REFS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?is)references\s+"?([\w.]+)"?"#).unwrap());

const SKIP: [&str; 6] = [
    "primary key",
    "foreign key",
    "constraint",
    "unique",
    "check",
    "--",
];

fn split_cols(body: &str) -> Vec<String> {
    let (mut depth, mut cur, mut out) = (0i32, String::new(), Vec::new());
    for ch in body.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if ch == ',' && depth == 0 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

pub fn parse_ddl(text: &str, into: &mut Tables) {
    for cap in CREATE.captures_iter(text) {
        let name = cap[1].trim_matches('"').to_string();
        let mut cols = Vec::new();
        for raw in split_cols(&cap[2]) {
            let line = raw.trim();
            let lower = line.to_lowercase();
            if line.is_empty() || SKIP.iter().any(|s| lower.starts_with(s)) {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(col), Some(ty)) = (parts.next(), parts.next()) else {
                continue;
            };
            cols.push(Column {
                name: col.trim_matches('"').to_string(),
                ty: ty.trim_end_matches(',').to_string(),
                pk: lower.contains("primary key"),
                fk: REFS
                    .captures(line)
                    .map(|c| c[1].trim_matches('"').to_string()),
            });
        }
        into.insert(name, cols);
    }
    for cap in ADD_COL.captures_iter(text) {
        into.entry(cap[1].trim_matches('"').to_string())
            .or_default()
            .push(Column {
                name: cap[2].to_string(),
                ty: cap[3].to_string(),
                pk: false,
                fk: None,
            });
    }
    for cap in DROP_COL.captures_iter(text) {
        if let Some(cols) = into.get_mut(cap[1].trim_matches('"')) {
            cols.retain(|c| c.name != cap[2]);
        }
    }
    for cap in DROP_TABLE.captures_iter(text) {
        into.shift_remove(cap[1].trim_matches('"'));
    }
}

pub fn destructive(text: &str) -> bool {
    DROP_COL.is_match(text) || DROP_TABLE.is_match(text)
}

/// Archivos de migracion de un servicio, en orden.
pub fn migrations_of(m: &Manifest) -> Vec<PathBuf> {
    let Some(path) = &m.infra.migrations else {
        return vec![];
    };
    let p = Path::new(path);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        m.origin.parent().unwrap_or(Path::new(".")).join(p)
    };
    if full.is_dir() {
        let mut files: Vec<_> = std::fs::read_dir(&full)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "sql"))
            .collect();
        files.sort();
        files
    } else if full.exists() {
        vec![full]
    } else {
        vec![]
    }
}

/// El esquema ES la suma de las migraciones plegadas en orden. No hay un
/// schema.sql duplicado que se desincronice.
pub fn schemas(manifests: &[Manifest]) -> IndexMap<String, Tables> {
    let mut out = IndexMap::new();
    for m in manifests {
        let files = migrations_of(m);
        if files.is_empty() {
            continue;
        }
        let mut tables = Tables::new();
        for f in files {
            if let Ok(text) = std::fs::read_to_string(&f) {
                parse_ddl(&text, &mut tables);
            }
        }
        out.insert(m.service.clone(), tables);
    }
    out
}

// ---------- helpers de nombres ----------

fn words(s: &str) -> Vec<&str> {
    s.split(['.', '@', '_', '-'])
        .filter(|w| !w.is_empty())
        .collect()
}

pub fn pascal(s: &str) -> String {
    words(s).iter().map(|w| upper1(w)).collect()
}

pub fn camel(s: &str) -> String {
    let w = words(s);
    match w.split_first() {
        Some((first, rest)) => {
            first.to_string() + &rest.iter().map(|w| upper1(w)).collect::<String>()
        }
        None => String::new(),
    }
}

fn upper1(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

pub fn tfname(s: &str) -> String {
    let out: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    out.split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

/// Pub/Sub no admite '@' en nombres de topic.
pub fn topic(ev: &str) -> String {
    ev.replace('@', ".")
}

pub fn ts_type(t: &str) -> &str {
    match t {
        "string" | "timestamp" | "uuid" => "string",
        "int" | "float" => "number",
        "bool" => "boolean",
        "money" => "{ amount: number; currency: string }",
        _ => "unknown",
    }
}
