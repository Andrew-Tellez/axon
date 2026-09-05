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

/// Que pasa con los eventos de este servicio cuando llegan a la bodega.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Analytics {
    /// `false` deja el servicio fuera de la exportacion.
    pub export: bool,
    /// Que hacer con los campos declarados `pii` al exportarlos:
    /// `"exclude"` no los manda, `"hash"` manda un SHA-256 con salt.
    ///
    /// El default es excluir. Una bodega es el lugar donde un dato personal
    /// vive mas tiempo, se copia mas veces y lo lee mas gente, asi que el
    /// valor seguro tiene que ser el que no lo manda.
    pub pii: String,
}

impl Default for Analytics {
    fn default() -> Self {
        Self {
            export: true,
            pii: "exclude".into(),
        }
    }
}

/// El pooler o sharder delante de la base.
///
/// Poner un proxy en el camino de datos cambia el sujeto de casi todas las
/// reglas de conexiones, y —lo mas importante— **rompe el aislamiento por
/// inquilino si nadie lo declara**: en modo transaccion la misma conexion
/// fisica se le entrega a otro inquilino, y una GUC de sesion que sobrevive
/// devuelve las filas del anterior sin un error.
///
/// De ahi que casi todo lo de aca sea obligatorio en vez de tener un default
/// comodo: la eleccion tiene que ser de alguien.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Pooler {
    /// `"none"` (sin pooler), o `"pgdog"`.
    pub engine: String,
    /// `transaction`, `session` o `statement`. En `transaction` la conexion se
    /// devuelve al pool en cada COMMIT, que es lo que rompe el aislamiento por
    /// sesion.
    pub mode: String,
    /// Nodos de reparto. `1` es solo pooler, sin sharding.
    pub shards: u32,
    /// Tope de conexiones de CLIENTE que acepta el pooler. Con un pooler en
    /// medio, la aritmetica de las instancias se compara contra esto y no
    /// contra el tope del motor.
    pub max_client_conn: Option<u32>,
    /// Conexiones que el pooler abre a CADA motor.
    pub pool_size: Option<u32>,
    /// Rechazar toda consulta que toque mas de un nodo, en vez de ejecutarla.
    /// Convierte cada limitacion del sharder en un error ruidoso.
    pub cross_shard_disabled: bool,
    /// Como se fija el inquilino: `"set_local"` es lo unico seguro en modo
    /// transaccion. Ver el encabezado que genera `axon rls`.
    pub tenant_binding: Option<String>,
}

impl Default for Pooler {
    fn default() -> Self {
        Self {
            engine: "none".into(),
            // el mas seguro de los tres, no el mas rapido
            mode: "session".into(),
            shards: 1,
            max_client_conn: None,
            pool_size: None,
            // fallar ruidosamente antes que ejecutar algo que el sharder no
            // sabe resolver bien
            cross_shard_disabled: true,
            tenant_binding: None,
        }
    }
}

impl Pooler {
    pub fn activo(&self) -> bool {
        self.engine != "none"
    }
}

/// Un feature flag.
///
/// Lo que aporta declararlos no es el SDK —OpenFeature y flagd ya existen—
/// sino que el compilador pueda imponer lo que nadie impone: que cada flag
/// tenga dueno y fecha de muerte. Un codigo con doscientos flags viejos no
/// tiene doscientas features: tiene doscientas ramas que nadie prueba.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Flag {
    pub owner: Option<String>,
    /// Variantes, como en OpenFeature: un flag no es solo un booleano. El
    /// valor puede ser bool, string, numero u objeto, y la evaluacion devuelve
    /// el valor de una variante con nombre.
    ///
    /// Sin `variants`, el flag es el caso booleano y las variantes son `on` y
    /// `off` — que es lo que necesita la mayoria y no vale la pena escribir.
    #[serde(default)]
    pub variants: IndexMap<String, serde_json::Value>,
    /// Nombre de la variante por defecto. Con variantes propias es obligatorio.
    pub default_variant: Option<String>,
    /// `YYYY-MM-DD`. Pasada esa fecha, `verify` falla: el flag se limpia o se
    /// renueva con una decision explicita.
    pub expires: Option<String>,
    /// El valor seguro. Un flag nuevo prendido por defecto no es un rollout.
    #[serde(default)]
    pub default: bool,
    /// Porcentaje del rollout gradual, 0..=100.
    pub rollout: Option<u32>,
    /// Campo por el que se fija la decision. Sin esto la evaluacion es por
    /// peticion, y la MISMA entidad cambia de camino a mitad de un flujo.
    pub sticky_by: Option<String>,
    /// Interruptor de emergencia: vive indefinidamente y no tiene rollout
    /// gradual, porque se apaga entero o no sirve.
    #[serde(default)]
    pub kill_switch: bool,
}

impl Flag {
    /// Las variantes efectivas. Un flag sin `variants` es el caso booleano.
    pub fn variantes(&self) -> IndexMap<String, serde_json::Value> {
        if self.variants.is_empty() {
            let mut v = IndexMap::new();
            v.insert("on".into(), serde_json::Value::Bool(true));
            v.insert("off".into(), serde_json::Value::Bool(false));
            v
        } else {
            self.variants.clone()
        }
    }

    /// La variante por defecto, o la que corresponda al booleano `default`.
    pub fn variante_defecto(&self) -> String {
        self.default_variant.clone().unwrap_or_else(|| {
            if self.default {
                "on".into()
            } else {
                "off".into()
            }
        })
    }

    /// El tipo de OpenFeature que corresponde a los valores declarados.
    /// Determina el accesor que se genera: `getBooleanValue`, `getStringValue`,
    /// `getNumberValue` o `getObjectValue`.
    pub fn tipo(&self) -> &'static str {
        match self.variantes().values().next() {
            Some(serde_json::Value::Bool(_)) | None => "boolean",
            Some(serde_json::Value::String(_)) => "string",
            Some(serde_json::Value::Number(_)) => "number",
            _ => "object",
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

/// Motores de almacen que axon soporta HOY.
///
/// Es una lista cerrada a proposito. Antes `state` era una cadena libre, asi
/// que `state = "neo4j"` pasaba `verify` sin un error y generaba una instancia
/// de Cloud SQL Postgres: salida incorrecta, en silencio, que es el peor modo
/// de fallo que existe.
///
/// El plan es soportar mas familias —series temporales, grafos, columnares,
/// documentales—, y el orden natural son las extensiones de Postgres
/// (TimescaleDB, Apache AGE, pgvector), porque reusan el parser SQL, las
/// migraciones, la RLS y los cuatro targets que ya existen. Hasta entonces,
/// declarar un motor que no esta aca tiene que fallar y decir como seguir.
pub const MOTORES: [&str; 1] = ["postgres"];

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
    /// Conexiones que abre CADA instancia. Multiplicado por el techo de
    /// instancias es lo que le llega al motor.
    pub pool_size: Option<u32>,
    /// Tope de conexiones del motor. Si el producto lo pasa, el servicio se
    /// cae por agotamiento cuando escala, no cuando lo pruebas.
    pub max_connections: Option<u32>,
    /// Alta disponibilidad: un standby con failover automatico.
    ///
    /// NO es lo mismo que una replica de lectura, y confundirlos es el error
    /// mas comun del tema. Del standby no se lee: existe para que el servicio
    /// siga en pie cuando el primario se cae, y por eso NO rompe la
    /// consistencia. De una replica de lectura si se lee, va con retraso, y
    /// por eso si la rompe.
    pub ha: Option<bool>,
    /// Dias de retencion de respaldos. Alta disponibilidad no es respaldo: un
    /// standby replica el `DROP TABLE` en segundos.
    pub backup_retention_days: Option<u32>,
    /// Recuperacion a un punto en el tiempo. Lo unico que salva de un borrado
    /// logico, que es de lo que un standby no salva.
    pub pitr: Option<bool>,
    /// Replicas de LECTURA: se lee de ellas, y van con retraso. Declararlas es
    /// elegir disponibilidad sobre consistencia para esas lecturas.
    pub read_replicas: Option<u32>,
    /// Columna por la que se reparte la tabla entre nodos. Toda tabla la
    /// necesita, y ninguna FK puede cruzar de una repartida a una que no.
    pub shard_key: Option<String>,
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

/// Un paso de una saga: la accion y lo que la deshace.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Paso {
    /// `servicio.metodo` a invocar. Tiene que ser una dependencia declarada.
    #[serde(rename = "do")]
    pub hacer: String,
    /// `servicio.metodo` que revierte el paso. Solo el ULTIMO puede omitirlo:
    /// si el ultimo falla, no hay nada suyo que deshacer.
    pub undo: Option<String>,
}

impl Paso {
    /// `("payments", "capturePayment")`. `None` si no tiene la forma
    /// `servicio.metodo`.
    pub fn partes(r: &str) -> Option<(&str, &str)> {
        let (svc, met) = r.split_once('.')?;
        (!svc.is_empty() && !met.is_empty() && !met.contains('.')).then_some((svc, met))
    }
}

/// Saga: una secuencia de pasos en servicios distintos, cada uno con su
/// compensacion, coordinada por este servicio.
///
/// Lo que la hace declarable es que el coordinador no tiene logica de negocio:
/// llama en orden, y si algo falla deshace en orden inverso lo que ya hizo. Eso
/// se genera. Lo que no se puede generar —que exista la compensacion, que sea
/// idempotente, que el presupuesto de tiempo cierre— se puede REFUTAR, y es
/// donde estan los errores que cuestan dinero.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Saga {
    /// Metodo propio o evento consumido que la arranca.
    pub on: Option<String>,
    #[serde(default)]
    pub steps: Vec<Paso>,
    /// Presupuesto del flujo completo. Tiene que cubrir la suma de los pasos:
    /// rendirse mientras un paso sigue en vuelo deja al coordinador
    /// compensando algo que despues tiene exito.
    pub timeout_ms: Option<u32>,
}

impl Saga {
    /// La tabla donde vive el avance. Sin ella un reinicio del coordinador
    /// pierde la saga a medias: ni termina ni compensa.
    pub fn tabla(nombre: &str) -> String {
        format!("saga_{}", nombre.to_lowercase())
    }
}

/// Event sourcing: el estado ES el flujo de eventos, y lo que hoy se guarda en
/// una fila es una proyeccion de ese flujo.
///
/// Lo que se puede generar de esto es todo lo mecanico: la tabla append-only, el
/// `fold` con un caso por evento declarado —asi que agregar un evento rompe la
/// compilacion— y el append con version optimista. Lo que se puede REFUTAR es lo
/// que cuesta caro: un evento que el servicio no emite, un `UPDATE` sobre el
/// flujo, o la falta del UNIQUE que evita que dos escrituras concurrentes se
/// pisen sin un solo error.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Aggregate {
    /// Los eventos que componen el estado. Todos tienen que estar declarados en
    /// `[emits]`: un agregado no puede fundarse en un contrato que no existe.
    #[serde(default)]
    pub events: Vec<String>,
    /// La maquina de estados que gobierna las transiciones, si la hay. Con
    /// ella, el `fold` generado rechaza el evento que llega fuera de orden en
    /// vez de aplicarlo.
    pub machine: Option<String>,
    /// Cada cuantos eventos se guarda una foto. 0 es sin fotos: reconstruir
    /// desde el principio siempre.
    #[serde(default)]
    pub snapshot_every: u32,
}

impl Aggregate {
    /// La tabla del flujo. Append-only: `verify` bloquea cualquier migracion
    /// que la actualice o borre de ella.
    pub fn tabla(nombre: &str) -> String {
        format!("{}_event", nombre.to_lowercase())
    }
    /// La tabla de fotos, si se declararon.
    pub fn fotos(nombre: &str) -> String {
        format!("{}_snapshot", nombre.to_lowercase())
    }
}

/// CQRS: un modelo de lectura construido aplicando eventos ya declarados.
///
/// Lo que aporta declararlo no es el codigo —una proyeccion es un `switch`—
/// sino que el compilador imponga lo que nadie impone: que la vista solo
/// consuma eventos que alguien emite, que tenga donde anotar hasta donde llego,
/// y que su obsolescencia quepa en la que el servicio ya prometio.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct View {
    /// Los eventos que la construyen.
    #[serde(default)]
    pub on: Vec<String>,
    /// La tabla donde vive. Por defecto `vista_<nombre>`.
    pub table: Option<String>,
    /// Cuanto puede atrasarse. Tiene que caber en el `max_staleness_ms` del
    /// servicio: una vista mas vieja que eso hace mentir a la declaracion.
    pub max_staleness_ms: Option<u32>,
}

impl View {
    pub fn tabla(&self, nombre: &str) -> String {
        self.table
            .clone()
            .unwrap_or_else(|| format!("vista_{}", nombre.to_lowercase()))
    }
    /// Donde anota hasta donde llego. Sin esto, un reinicio reprocesa desde el
    /// principio o se salta lo que no alcanzo a aplicar, y las dos cosas dan
    /// una vista incorrecta sin un error.
    pub fn checkpoint(nombre: &str) -> String {
        format!("vista_{}_checkpoint", nombre.to_lowercase())
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
    /// Feature flags del servicio.
    #[serde(default)]
    pub flags: IndexMap<String, Flag>,
    /// Exportacion a la bodega de datos.
    #[serde(default)]
    pub analytics: Analytics,
    /// Pooler o sharder delante de la base. Ver `Pooler`.
    #[serde(default)]
    pub pooler: Pooler,
    /// Maquinas de estado del dominio: la unica logica de negocio que vale
    /// la pena declarar, porque es la misma en todos los lenguajes.
    #[serde(default)]
    pub machine: IndexMap<String, Machine>,
    /// Sagas que coordina este servicio. Ver `Saga`.
    #[serde(default)]
    pub saga: IndexMap<String, Saga>,
    /// Agregados con event sourcing. Ver `Aggregate`.
    #[serde(default)]
    pub aggregate: IndexMap<String, Aggregate>,
    /// Modelos de lectura. Ver `View`.
    #[serde(default)]
    pub view: IndexMap<String, View>,
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
        if o.pool_size.is_some() {
            out.infra.pool_size = o.pool_size;
        }
        if o.max_connections.is_some() {
            out.infra.max_connections = o.max_connections;
        }
        if o.read_replicas.is_some() {
            out.infra.read_replicas = o.read_replicas;
        }
        if o.ha.is_some() {
            out.infra.ha = o.ha;
        }
        if o.backup_retention_days.is_some() {
            out.infra.backup_retention_days = o.backup_retention_days;
        }
        if o.pitr.is_some() {
            out.infra.pitr = o.pitr;
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
    /// Genera su valor de una secuencia: `serial`, `bigserial`,
    /// `GENERATED ... AS IDENTITY`. Al repartir, cada nodo tiene su propia
    /// secuencia y los valores colisionan.
    pub serial: bool,
}

/// Una tabla: sus columnas y sus restricciones de unicidad.
///
/// Las unicidades van como conjuntos de columnas y no como una marca por
/// columna, porque eso es lo que decide si son seguras al repartir: una
/// `UNIQUE (tenant_id, handle)` la puede garantizar cada nodo por separado;
/// una `UNIQUE (handle)` no, y el conjunto de nodos tampoco.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Tabla {
    pub cols: Vec<Column>,
    /// Cada entrada es el conjunto de columnas de una restriccion UNIQUE o
    /// PRIMARY KEY.
    pub uniques: Vec<Vec<String>>,
}

impl Tabla {
    pub fn col(&self, nombre: &str) -> Option<&Column> {
        self.cols.iter().find(|c| c.name == nombre)
    }
    pub fn tiene(&self, nombre: &str) -> bool {
        self.col(nombre).is_some()
    }
}

pub type Tables = IndexMap<String, Tabla>;

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
                let mut uniques: Vec<Vec<String>> = Vec::new();
                let mut cols: Vec<Column> = Vec::new();
                for c in &ct.columns {
                    let n = ident(&c.name);
                    let t = tipo_sql(&c.data_type);
                    let pk = c
                        .options
                        .iter()
                        .any(|o| matches!(o.option, ColumnOption::PrimaryKey(_)));
                    // UNIQUE y PRIMARY KEY a nivel de columna son unicidades
                    // de una sola columna
                    let uniq = c
                        .options
                        .iter()
                        .any(|o| matches!(o.option, ColumnOption::Unique { .. }));
                    if pk || uniq {
                        uniques.push(vec![n.clone()]);
                    }
                    cols.push(Column {
                        // `serial` y `bigserial` son azucar de Postgres para una
                        // secuencia, y `GENERATED AS IDENTITY` tambien
                        serial: t.contains("serial")
                            || c.options
                                .iter()
                                .any(|o| matches!(o.option, ColumnOption::Generated { .. })),
                        name: n,
                        ty: t,
                        pk,
                        fk: c.options.iter().find_map(|o| match &o.option {
                            ColumnOption::ForeignKey(f) => Some(nombre_tabla(&f.foreign_table)),
                            _ => None,
                        }),
                    });
                }
                // las mismas restricciones, declaradas a nivel de tabla
                for c in &ct.constraints {
                    match c {
                        TableConstraint::PrimaryKey(pk) => {
                            let cs: Vec<String> = pk
                                .columns
                                .iter()
                                .map(|k| k.to_string().trim_matches('"').to_lowercase())
                                .collect();
                            for k in &cs {
                                marcar(&mut cols, k, |c| c.pk = true);
                            }
                            uniques.push(cs);
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
                        TableConstraint::Unique(u) => {
                            uniques.push(
                                u.columns
                                    .iter()
                                    .map(|k| k.to_string().trim_matches('"').to_lowercase())
                                    .collect(),
                            );
                        }
                        _ => {}
                    }
                }
                into.insert(nombre_tabla(&ct.name), Tabla { cols, uniques });
            }
            Statement::AlterTable(at) => {
                let tabla = nombre_tabla(&at.name);
                for op in at.operations {
                    match op {
                        AlterTableOperation::AddColumn { column_def, .. } => {
                            let n = ident(&column_def.name);
                            let t = tipo_sql(&column_def.data_type);
                            let uniq = column_def
                                .options
                                .iter()
                                .any(|o| matches!(o.option, ColumnOption::Unique { .. }));
                            let entrada = into.entry(tabla.clone()).or_default();
                            if uniq {
                                entrada.uniques.push(vec![n.clone()]);
                            }
                            entrada.cols.push(Column {
                                serial: t.contains("serial")
                                    || column_def.options.iter().any(|o| {
                                        matches!(o.option, ColumnOption::Generated { .. })
                                    }),
                                name: n,
                                ty: t,
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
                            if let Some(tb) = into.get_mut(&tabla) {
                                let fuera: Vec<String> = column_names.iter().map(ident).collect();
                                tb.cols.retain(|c| !fuera.contains(&c.name));
                                // una unicidad sobre una columna borrada ya no existe
                                tb.uniques.retain(|u| !u.iter().any(|c| fuera.contains(c)));
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

/// Normaliza un nombre de campo para comparar: minusculas y sin separadores.
///
/// El mismo concepto se escribe distinto en cada capa —`customerEmail` en el
/// contrato, `customer_email` en la base, `customer-email` en una cabecera— y
/// declararlo tres veces en `pii` seria absurdo. Se declara una y se compara
/// normalizado.
pub fn normalizar(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Si un campo esta declarado como personal, comparando normalizado.
pub fn es_pii(declarados: &[String], campo: &str) -> bool {
    let c = normalizar(campo);
    declarados.iter().any(|d| normalizar(d) == c)
}

#[cfg(test)]
mod pii {
    use super::*;

    #[test]
    fn el_mismo_concepto_se_declara_una_vez() {
        let d = vec!["customer_email".to_string()];
        for campo in [
            "customerEmail",
            "customer_email",
            "CustomerEmail",
            "customer-email",
        ] {
            assert!(es_pii(&d, campo), "{campo} deberia coincidir");
        }
        for campo in ["customer_id", "email_template", "customer"] {
            assert!(!es_pii(&d, campo), "{campo} no deberia coincidir");
        }
    }
}

/// Fecha de hoy como (ano, mes, dia), sin dependencias.
///
/// Es el algoritmo civil_from_days de Howard Hinnant: los dias desde la epoca
/// se corren a una era que empieza en marzo, y ahi el patron de meses es
/// regular. Una dependencia entera para comparar dos fechas seria mucho.
pub fn hoy() -> (i64, i64, i64) {
    let dias = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 / 86_400)
        .unwrap_or(0);
    civil(dias)
}

fn civil(dias_epoca: i64) -> (i64, i64, i64) {
    let z = dias_epoca + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY-MM-DD` a una tupla comparable. `None` si no tiene esa forma.
pub fn fecha(s: &str) -> Option<(i64, i64, i64)> {
    let mut p = s.trim().split('-');
    let a = p.next()?.parse().ok()?;
    let m = p.next()?.parse().ok()?;
    let d = p.next()?.parse().ok()?;
    if p.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some((a, m, d))
}

#[cfg(test)]
mod fechas {
    use super::*;

    #[test]
    fn el_calendario_civil_es_correcto() {
        // dias conocidos desde la epoca, incluidos bisiestos y siglos
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(1), (1970, 1, 2));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(civil(11_016), (2000, 2, 29)); // bisiesto de un siglo divisible por 400
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(20_608), (2026, 6, 4));
    }

    #[test]
    fn hoy_es_una_fecha_razonable() {
        let (a, m, d) = hoy();
        assert!((2025..2100).contains(&a), "ano fuera de rango: {a}");
        assert!((1..=12).contains(&m));
        assert!((1..=31).contains(&d));
    }

    #[test]
    fn parsear_fechas() {
        assert_eq!(fecha("2026-12-31"), Some((2026, 12, 31)));
        assert_eq!(fecha(" 2026-01-02 "), Some((2026, 1, 2)));
        assert_eq!(fecha("2026-13-01"), None);
        assert_eq!(fecha("2026-12"), None);
        assert_eq!(fecha("manana"), None);
    }
}
