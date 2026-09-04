//! Carga, descubrimiento y esquema derivado de migraciones.
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
    /// Quien puede llamarla desde el edge: "public" o "required". No tiene
    /// default a proposito: una ruta expuesta sin decidir esto es un incidente.
    pub auth: Option<String>,
    /// Peticiones por minuto en el gateway.
    pub rate_limit: Option<u32>,
    /// Presupuesto de tiempo en el edge.
    pub timeout_ms: Option<u32>,
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

/// El lado del teorema CAP que elige este servicio.
///
/// La tolerancia a particiones no se elige: en un sistema distribuido la red
/// se parte y punto. Lo que se elige es que hacer mientras esta partida, y esa
/// decision cambia el nivel de aislamiento, la topologia de lectura y si el
/// codigo generado te obliga a escribir un camino degradado.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Cap {
    /// "strong" (CP): antes de servir un dato viejo, no sirve nada.
    /// "eventual" (AP): sirve algo viejo antes que no servir nada.
    pub consistency: String,
    /// "reject" falla cerrado; "degrade" obliga a declarar que se sirve.
    pub on_partition: String,
    /// Presupuesto de obsolescencia. Sin un numero, "eventual" no significa nada.
    pub max_staleness_ms: Option<u32>,
    /// `true` cuando la eleccion la hizo alguien; `false` cuando es el default.
    #[serde(skip)]
    pub declarado: bool,
}

impl Default for Cap {
    fn default() -> Self {
        // El par seguro: falla cerrado. Es un default, no una decision, y
        // `verify` avisa de que nadie la tomo.
        Self {
            consistency: "strong".into(),
            on_partition: "reject".into(),
            max_staleness_ms: None,
            declarado: false,
        }
    }
}

impl Cap {
    pub fn eventual(&self) -> bool {
        self.consistency == "eventual"
    }
    pub fn degrada(&self) -> bool {
        self.on_partition == "degrade"
    }
    /// Aislamiento acorde: pagar dos veces sale mas caro que reintentar.
    pub fn aislamiento(&self) -> &str {
        if self.eventual() {
            "READ COMMITTED"
        } else {
            "SERIALIZABLE"
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Patterns {
    #[serde(default)]
    pub outbox: bool,
}

/// Un bucket del servicio. `public = true` lo pone detras de un CDN: nada
/// se sirve publico sin cache, y nada privado la lleva.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Bucket {
    #[serde(default)]
    pub public: bool,
    /// Dias tras los que el objeto se borra. Sin esto un bucket crece para siempre.
    pub retention_days: Option<u32>,
    /// TTL del CDN en segundos; solo aplica a buckets publicos.
    pub cache_ttl: Option<u32>,
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
    /// Almacenamiento de objetos del servicio, por nombre logico.
    #[serde(default)]
    pub buckets: IndexMap<String, Bucket>,
    /// Columna que identifica al inquilino. Si esta, toda tabla la necesita y
    /// `axon rls` genera la politica que la aplica.
    pub tenant_column: Option<String>,
    /// Tablas que no son de negocio y no llevan inquilino.
    #[serde(default)]
    pub tenant_exempt: Vec<String>,
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
    /// Nombres de campo que llevan datos personales, en cualquier evento o
    /// metodo de este servicio. Lo usa el generador para redactar logs y
    /// `verify` para bloquear que salgan por una ruta publica.
    #[serde(default)]
    pub pii: Vec<String>,
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
    /// Lado del teorema CAP. Ver `Cap`.
    #[serde(default)]
    pub cap: Cap,
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
        if o.max_instances.is_some() {
            out.infra.max_instances = o.max_instances;
        }
        if o.port.is_some() {
            out.infra.port = o.port;
        }
        if !o.buckets.is_empty() {
            out.infra.buckets = o.buckets.clone();
        }
        if o.tenant_column.is_some() {
            out.infra.tenant_column = o.tenant_column.clone();
        }
        if !o.tenant_exempt.is_empty() {
            out.infra.tenant_exempt = o.tenant_exempt.clone();
        }
    }
    out
}

pub fn load(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut m: Manifest = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    // serde no distingue "ausente" de "igual al default"; el texto si
    m.cap.declarado = text.contains("[cap]");
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

use sqlparser::ast::{AlterTableOperation, ColumnOption, ObjectType, Statement, TableConstraint};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

/// El esquema se lee con un parser SQL de verdad. Una regex se rompe con
/// `PARTITION BY`, tipos compuestos o lo que genere cualquier ORM, y lo peor
/// es que se rompe en silencio: devuelve columnas mal y nadie se entera.
/// Esto falla ruidosamente, que es lo unico aceptable para el ER y para el
/// chequeo de FK entre servicios.
fn statements(text: &str, origen: &str) -> Vec<Statement> {
    match Parser::parse_sql(&PostgreSqlDialect {}, text) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "axon: {origen}: no se pudo parsear el SQL: {e}\n      \
                 axon lee las migraciones para el ER y para bloquear FK entre \
                 servicios; preferir fallar a adivinar columnas"
            );
            std::process::exit(1);
        }
    }
}

/// Postgres pliega a minusculas todo identificador sin comillas. Guardar el
/// casing del archivo y despues citarlo produce una columna que no existe.
fn ident(i: &sqlparser::ast::Ident) -> String {
    match i.quote_style {
        Some(_) => i.value.clone(),
        None => i.value.to_lowercase(),
    }
}

fn nombre_tabla(o: &sqlparser::ast::ObjectName) -> String {
    o.0.last()
        .map(|p| {
            let t = p.to_string();
            match t.starts_with('"') {
                true => t.trim_matches('"').to_string(),
                false => t.to_lowercase(),
            }
        })
        .unwrap_or_default()
}

/// Un tipo tiene que caber en un token: el ER de mermaid es `<tipo> <columna>`.
fn tipo_sql(t: &sqlparser::ast::DataType) -> String {
    t.to_string().to_lowercase().replace(' ', "_")
}

pub fn parse_ddl(text: &str, origen: &str, into: &mut Tables) {
    for st in statements(text, origen) {
        match st {
            Statement::CreateTable(ct) => {
                let mut cols: Vec<Column> = ct
                    .columns
                    .iter()
                    .map(|c| Column {
                        name: ident(&c.name),
                        ty: tipo_sql(&c.data_type),
                        pk: c
                            .options
                            .iter()
                            .any(|o| matches!(o.option, ColumnOption::PrimaryKey(_))),
                        fk: c.options.iter().find_map(|o| match &o.option {
                            ColumnOption::ForeignKey(f) => Some(nombre_tabla(&f.foreign_table)),
                            _ => None,
                        }),
                    })
                    .collect();
                // las mismas restricciones, declaradas a nivel de tabla
                for c in &ct.constraints {
                    match c {
                        TableConstraint::PrimaryKey(pk) => {
                            for k in &pk.columns {
                                marcar(&mut cols, &k.to_string().to_lowercase(), |c| c.pk = true);
                            }
                        }
                        TableConstraint::ForeignKey(fk) => {
                            let t = nombre_tabla(&fk.foreign_table);
                            for k in &fk.columns {
                                let t = t.clone();
                                marcar(&mut cols, &k.to_string().to_lowercase(), move |c| {
                                    c.fk = Some(t.clone())
                                });
                            }
                        }
                        _ => {}
                    }
                }
                into.insert(nombre_tabla(&ct.name), cols);
            }
            Statement::AlterTable(at) => {
                let tabla = nombre_tabla(&at.name);
                for op in at.operations {
                    match op {
                        AlterTableOperation::AddColumn { column_def, .. } => {
                            into.entry(tabla.clone()).or_default().push(Column {
                                name: ident(&column_def.name),
                                ty: tipo_sql(&column_def.data_type),
                                pk: false,
                                fk: column_def.options.iter().find_map(|o| match &o.option {
                                    ColumnOption::ForeignKey(f) => {
                                        Some(nombre_tabla(&f.foreign_table))
                                    }
                                    _ => None,
                                }),
                            });
                        }
                        AlterTableOperation::DropColumn { column_names, .. } => {
                            if let Some(cols) = into.get_mut(&tabla) {
                                let fuera: Vec<String> = column_names.iter().map(ident).collect();
                                cols.retain(|c| !fuera.contains(&c.name));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Statement::Drop {
                object_type: ObjectType::Table,
                names,
                ..
            } => {
                for n in names {
                    into.shift_remove(&nombre_tabla(&n));
                }
            }
            _ => {}
        }
    }
}

fn marcar(cols: &mut [Column], nombre: &str, f: impl Fn(&mut Column)) {
    let nombre = nombre.trim_matches('"');
    if let Some(c) = cols.iter_mut().find(|c| c.name == nombre) {
        f(c);
    }
}

/// Destructivo de verdad, no "contiene la palabra DROP en un comentario".
pub fn destructive(text: &str, origen: &str) -> bool {
    statements(text, origen).iter().any(|st| match st {
        Statement::Drop {
            object_type: ObjectType::Table,
            ..
        } => true,
        Statement::AlterTable(at) => at
            .operations
            .iter()
            .any(|o| matches!(o, AlterTableOperation::DropColumn { .. })),
        _ => false,
    })
}

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
                parse_ddl(&text, &f.display().to_string(), &mut tables);
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
