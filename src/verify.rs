//! Drift: lo que convierte al manifiesto en algo mas que documentacion.
use crate::manifest::*;
use indexmap::IndexMap;

/// Gobernanza: reglas del equipo, versionadas junto al codigo.
/// Sin `axon.policy.toml` aplican estos valores por defecto.
#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct Policy {
    pub require_owner: bool,
    pub require_tier: bool,
    pub allowed_event_prefixes: Vec<String>,
    pub max_deps_per_service: usize,
    /// El layout del repo es del equipo, no de axon. Sin esto, `axon ci`
    /// tendria que adivinarlo, y adivinar es lo que lo volvia inservible.
    pub ci: Ci,
}

/// `{service}` se reemplaza por el nombre del servicio.
#[derive(Debug, serde::Deserialize)]
#[serde(default)]
pub struct Ci {
    pub manifests_dir: String,
    pub service_dir: String,
    pub test_cmd: String,
    pub contracts_path: String,
    pub image: String,
}

impl Default for Ci {
    fn default() -> Self {
        Self {
            manifests_dir: "manifests".into(),
            service_dir: "services/{service}".into(),
            test_cmd: "make -C services/{service} test".into(),
            contracts_path: "services/{service}/src/contracts.ts".into(),
            image: "${{ vars.REGISTRY }}/{service}@${{ steps.imagen.outputs.digest }}".into(),
        }
    }
}

impl Ci {
    pub fn para(&self, campo: &str, service: &str) -> String {
        campo.replace("{service}", service)
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            require_owner: true,
            require_tier: true,
            allowed_event_prefixes: vec![],
            max_deps_per_service: 7, // acoplamiento sincrono: si pasa de esto, es un monolito distribuido
            ci: Ci::default(),
        }
    }
}

pub fn load_policy(dir: &std::path::Path) -> Policy {
    let f = dir.join("axon.policy.toml");
    std::fs::read_to_string(f)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Heuristica corta y deliberadamente conservadora: solo formas que no pueden
/// ser el nombre de una variable de entorno.
fn parece_secreto(v: &str) -> bool {
    let t = v.trim();
    t.starts_with("sk_")
        || t.starts_with("AKIA")
        || t.starts_with("ghp_")
        || t.starts_with("-----BEGIN")
        || (t.len() > 32 && t.chars().any(|c| c.is_lowercase()) && t.contains(['/', '+', '=']))
}

/// Un placeholder no es un valor. Sin esto, `axon import` produciria
/// manifiestos que pasan verify sin que nadie los haya revisado.
fn pendiente(v: &Option<String>) -> bool {
    match v {
        None => true,
        Some(s) => {
            let s = s.trim().to_uppercase();
            s.is_empty() || s == "TODO" || s == "FIXME" || s == "TBD"
        }
    }
}

pub fn verify(ms: &[Manifest], pol: &Policy) -> Report {
    let (mut errors, mut warnings) = (Vec::new(), Vec::new());

    // gobernanza: nada sin dueno, nada sin criticidad, nombres bajo control
    for m in ms.iter().filter(|m| !m.external) {
        if pol.require_owner && pendiente(&m.owner) {
            errors.push(format!(
                "{}: sin `owner`; un servicio sin dueno no se despliega",
                m.service
            ));
        }
        if pol.require_tier && pendiente(&m.tier) {
            errors.push(format!(
                "{}: sin `tier`; la criticidad decide alertas y SLO",
                m.service
            ));
        }
        match m.infra.runtime.as_deref() {
            None | Some("container") => {}
            Some(other) => errors.push(format!(
                "{}: `runtime = \"{other}\"` no existe; hoy solo hay `container`. \
                 Otro modelo de ejecucion se agrega con un `axon-infra-*`",
                m.service
            )),
        }
        if m.depends.len() > pol.max_deps_per_service {
            warnings.push(format!(
                "{}: {} dependencias sincronas (limite {}); revisa si algo deberia ser un evento",
                m.service,
                m.depends.len(),
                pol.max_deps_per_service
            ));
        }
        if !pol.allowed_event_prefixes.is_empty() {
            for ev in m.emits.keys() {
                let pre = ev.split('.').next().unwrap_or("");
                if !pol.allowed_event_prefixes.iter().any(|p| p == pre) {
                    errors.push(format!(
                        "{ev}: prefijo `{pre}` fuera del catalogo de dominios permitido",
                    ));
                }
            }
        }
    }

    // un evento, un dueno, un esquema
    let mut emitters: IndexMap<&str, (&str, &Fields)> = IndexMap::new();
    for m in ms {
        for (ev, fields) in &m.emits {
            if let Some((owner, prev)) = emitters.get(ev.as_str()) {
                if *prev != fields {
                    errors.push(format!(
                        "{ev}: dos emisores con esquemas distintos ({owner} vs {})",
                        m.service
                    ));
                }
            }
            emitters.insert(ev, (&m.service, fields));
        }
    }

    // ---- seguridad. Cada regla cita su categoria del OWASP Top 10 (2021),
    // porque un error que no dice por que importa se silencia con un allow.
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        let pii: Vec<String> = m.pii.iter().map(|p| p.to_lowercase()).collect();

        for (name, meth) in &m.methods {
            let publico = meth.auth.as_deref() == Some("public");
            // A01: control de acceso roto. Una ruta publica que muta en un
            // servicio critico no es una decision que se toma sin pensarla.
            if publico && meth.mutating() && m.tier.as_deref() == Some("0") {
                errors.push(format!(
                    "[A01] {svc}.{name}: ruta publica que muta en un servicio tier 0; \
                     ponla detras de `auth = \"required\"` o baja el tier con criterio"
                ));
            }
            // A04: diseno inseguro. Sin presupuesto de tiempo, una ruta
            // publica es un agotamiento de recursos gratis.
            if publico && meth.timeout_ms.is_none() {
                errors.push(format!(
                    "[A04] {svc}.{name}: ruta publica sin `timeout_ms`; una peticion sin \
                     limite de tiempo es agotamiento de recursos"
                ));
            }
            // A09: fallos de registro. Un dato personal que sale por una ruta
            // publica termina en un log, una cache y un CDN.
            if publico {
                for campo in meth.output.keys() {
                    if pii.contains(&campo.to_lowercase()) {
                        errors.push(format!(
                            "[A09] {svc}.{name}: devuelve `{campo}`, declarado PII, por una ruta publica"
                        ));
                    }
                }
            }
        }

        // A02: fallos criptograficos. Un secreto en el manifiesto ya esta en
        // el historial de git; declarar su nombre es lo unico que corresponde.
        for k in &m.infra.secrets {
            if parece_secreto(k) {
                errors.push(format!(
                    "[A02] {svc}: `{k}` en `secrets` parece el valor y no el nombre; \
                     ahi va la referencia, el valor vive en el vault"
                ));
            }
        }

        // A05: mala configuracion. Un bucket publico sin retencion crece para
        // siempre y nadie sabe que hay dentro.
        for (nombre, b) in &m.infra.buckets {
            if b.public && b.retention_days.is_none() {
                warnings.push(format!(
                    "[A05] {svc}: bucket `{nombre}` publico y sin `retention_days`; \
                     nadie va a saber que quedo expuesto"
                ));
            }
        }
    }

    // A01: la tabla que se olvida de la columna del inquilino no recibe
    // politica, y una tabla sin politica no falla: devuelve las filas de todos.
    let esquemas = schemas(ms);
    for m in ms.iter().filter(|m| !m.external) {
        let Some(tenant) = &m.infra.tenant_column else {
            continue;
        };
        let Some(tablas) = esquemas.get(&m.service) else {
            continue;
        };
        for (t, cols) in tablas {
            if ["outbox", "inbox_seen"].contains(&t.as_str()) || m.infra.tenant_exempt.contains(t) {
                continue;
            }
            if !cols.iter().any(|c| &c.name == tenant) {
                errors.push(format!(
                    "[A01] {}.{t}: sin la columna `{tenant}`; se queda sin politica RLS y \
                     devuelve filas de todos los inquilinos. Agregala o ponla en `tenant_exempt`",
                    m.service
                ));
            }
        }
    }

    // ---- feature flags: lo que nadie impone ----
    let ahora = hoy();
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        for (nombre, f) in &m.flags {
            if f.owner.is_none() {
                errors.push(format!(
                    "{svc}.{nombre}: flag sin `owner`. El que lo prendio es el que lo apaga"
                ));
            }
            // Un codigo con doscientos flags viejos no tiene doscientas
            // features: tiene doscientas ramas que nadie prueba.
            match (&f.expires, f.kill_switch) {
                (None, false) => errors.push(format!(
                    "{svc}.{nombre}: flag sin `expires`. Un flag sin fecha de muerte no muere; \
                     si de verdad es permanente, declaralo `kill_switch = true`"
                )),
                (Some(_), true) => warnings.push(format!(
                    "{svc}.{nombre}: `kill_switch` con `expires`; un interruptor de emergencia \
                     vive mientras exista lo que apaga"
                )),
                (Some(e), false) => match fecha(e) {
                    None => errors.push(format!(
                        "{svc}.{nombre}: `expires = \"{e}\"` no tiene la forma YYYY-MM-DD"
                    )),
                    Some(f) if f < ahora => errors.push(format!(
                        "{svc}.{nombre}: vencio el {e}. O se limpia la rama muerta, o se renueva \
                         la fecha con una decision explicita: dejarlo vencido no es ninguna de las dos"
                    )),
                    _ => {}
                },
                _ => {}
            }

            // Un rollout por peticion hace que la MISMA entidad tome un camino
            // en una llamada y el otro en la siguiente. Con estado de por
            // medio, eso deja datos a medio migrar.
            // el orden importa: un kill switch con rollout es un error propio,
            // no un caso del sticky que falta
            match f.rollout {
                Some(_) if f.kill_switch => errors.push(format!(
                    "{svc}.{nombre}: `kill_switch` con `rollout`. Un interruptor de emergencia se \
                     apaga entero o no sirve de nada"
                )),
                Some(p) if p > 100 => errors.push(format!(
                    "{svc}.{nombre}: `rollout = {p}` no es un porcentaje"
                )),
                Some(p) if p > 0 && p < 100 && f.sticky_by.is_none() => errors.push(format!(
                    "{svc}.{nombre}: rollout al {p}% sin `sticky_by`. Evaluado por peticion, la \
                     misma entidad toma un camino y despues el otro, y queda a medio migrar"
                )),
                _ => {}
            }
            // Una variante por defecto que no existe hace que la evaluacion
            // caiga siempre al valor del codigo, y el flag deja de servir en
            // silencio: se ve como "el rollout no hace nada".
            let variantes = f.variantes();
            let defecto = f.variante_defecto();
            if !variantes.contains_key(&defecto) {
                errors.push(format!(
                    "{svc}.{nombre}: `default_variant = \"{defecto}\"` no esta en `variants` \
                     ({}). La evaluacion caeria siempre al valor del codigo",
                    variantes.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            // Mezclar tipos entre variantes rompe la evaluacion: OpenFeature
            // resuelve un tipo por flag, no uno por variante.
            let tipos: std::collections::BTreeSet<&str> = variantes
                .values()
                .map(|v| match v {
                    serde_json::Value::Bool(_) => "boolean",
                    serde_json::Value::String(_) => "string",
                    serde_json::Value::Number(_) => "number",
                    _ => "object",
                })
                .collect();
            if tipos.len() > 1 {
                errors.push(format!(
                    "{svc}.{nombre}: las variantes mezclan tipos ({}). OpenFeature resuelve un \
                     tipo por flag, no uno por variante",
                    tipos.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
            if f.default && !f.kill_switch {
                warnings.push(format!(
                    "{svc}.{nombre}: `default = true` en un flag que no es kill switch. Un flag \
                     nuevo prendido por defecto no es un rollout gradual: es un despliegue"
                ));
            }
            // El campo por el que se fija tiene que existir en algun contrato,
            // o la decision se fija por un dato que el servicio no recibe.
            if let Some(campo) = &f.sticky_by {
                let conocido = m.infra.tenant_column.as_deref() == Some(campo.as_str())
                    || m.methods.values().any(|me| me.input.contains_key(campo))
                    || m.emits.values().any(|fs| fs.contains_key(campo))
                    || ms.iter().any(|o| {
                        m.consumes
                            .keys()
                            .any(|ev| o.emits.get(ev).is_some_and(|fs| fs.contains_key(campo)))
                    });
                if !conocido {
                    errors.push(format!(
                        "{svc}.{nombre}: se fija por `{campo}`, que no aparece en ningun contrato \
                         ni es la columna del inquilino; el servicio no lo recibe"
                    ));
                }
            }
        }
    }

    // ---- escalado de la base: aritmetica sobre lo declarado ----
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        let inf = &m.infra;
        if inf.state.is_none() {
            if inf.pool_size.is_some() || inf.read_replicas.is_some() {
                errors.push(format!(
                    "{svc}: declara pool o replicas sin `state`; no tiene base propia"
                ));
            }
            continue;
        }

        // El agotamiento de conexiones no aparece cuando lo probas con una
        // instancia: aparece el dia que escala. Es una multiplicacion, y nadie
        // la hace.
        if let (Some(pool), Some(tope)) = (inf.pool_size, inf.max_connections) {
            let techo = inf.max_instances.unwrap_or(10);
            let pico = pool * techo;
            // el relay del outbox y las migraciones tambien abren conexiones
            let reservado = if m.patterns.outbox { 5 } else { 2 };
            if pico + reservado > tope {
                errors.push(format!(
                    "{svc}: {pool} conexiones x {techo} instancias = {pico}, mas {reservado} \
                     reservadas, supera el tope de {tope}. El servicio se cae por agotamiento \
                     cuando escale, no cuando lo pruebes: baja el pool, baja max_instances, \
                     o pon un pooler delante"
                ));
            } else if pico * 2 > tope {
                warnings.push(format!(
                    "{svc}: al pico usa {pico} de {tope} conexiones; queda poco margen para \
                     migraciones, un pooler o un segundo servicio en la misma instancia"
                ));
            }
        }
        if inf.pool_size.is_some() != inf.max_connections.is_some() {
            warnings.push(format!(
                "{svc}: declara `pool_size` o `max_connections` pero no el otro; sin los dos \
                 no se puede comprobar el agotamiento"
            ));
        }

        // Un tier 0 sin failover no es un tier 0: es una declaracion de
        // intenciones sin nada detras.
        let tier0 = m.tier.as_deref() == Some("0");
        if tier0 && inf.ha != Some(true) {
            errors.push(format!(
                "{svc}: `tier = \"0\"` sin `ha = true`. Un servicio critico con una sola \
                 instancia de base cae con ella: o declara el standby, o baja el tier"
            ));
        }
        // Alta disponibilidad no es respaldo: un standby replica el DROP TABLE
        // en segundos. Son dos problemas distintos con dos soluciones distintas.
        match inf.backup_retention_days {
            None if tier0 => errors.push(format!(
                "{svc}: `tier = \"0\"` sin `backup_retention_days`. Alta disponibilidad no es \
                 respaldo: el standby replica un borrado en segundos"
            )),
            Some(d) if d < 7 && tier0 => errors.push(format!(
                "{svc}: {d} dias de respaldo en un tier 0. Un borrado logico se descubre \
                 despues del fin de semana, no en el minuto siguiente"
            )),
            _ => {}
        }
        if inf.pitr == Some(true) && inf.backup_retention_days.is_none() {
            errors.push(format!(
                "{svc}: `pitr` sin `backup_retention_days`. Recuperar a un punto en el tiempo \
                 necesita un respaldo base desde el que avanzar"
            ));
        }
        if inf.ha.is_some() && inf.state.is_none() {
            errors.push(format!(
                "{svc}: declara `ha` sin `state`; no tiene base propia"
            ));
        }

        // Una replica va con retraso. Leer de ella y prometer consistencia
        // fuerte es la contradiccion del teorema, escrita en dos lugares.
        if inf.read_replicas.unwrap_or(0) > 0 && !m.cap.eventual() {
            errors.push(format!(
                "{svc}: lee de {} replicas y declara `consistency = \"strong\"`. Una replica \
                 va con retraso: o las lecturas son `eventual`, o no se leen de ahi",
                inf.read_replicas.unwrap_or(0)
            ));
        }
    }

    // ---- reparto entre nodos: la tabla que se olvida la clave no se puede repartir ----
    let esquemas_shard = schemas(ms);
    for m in ms.iter().filter(|m| !m.external) {
        let Some(clave) = &m.infra.shard_key else {
            continue;
        };
        let Some(tablas) = esquemas_shard.get(&m.service) else {
            continue;
        };
        let repartidas: Vec<&String> = tablas
            .iter()
            .filter(|(_, cols)| cols.iter().any(|c| &c.name == clave))
            .map(|(t, _)| t)
            .collect();
        for (t, cols) in tablas {
            if ["outbox", "inbox_seen"].contains(&t.as_str()) {
                continue;
            }
            if !repartidas.contains(&t) {
                errors.push(format!(
                    "{}.{t}: sin la columna `{clave}`, asi que no se puede repartir. Agregala \
                     o saca la tabla del esquema repartido",
                    m.service
                ));
                continue;
            }
            for c in cols {
                let Some(fk) = &c.fk else { continue };
                if tablas.contains_key(fk) && !repartidas.contains(&fk) {
                    errors.push(format!(
                        "{}.{t}.{}: FK a `{fk}`, que no lleva `{clave}`. Una FK entre una tabla \
                         repartida y una que no lo esta cruza nodos, y eso no se puede garantizar",
                        m.service, c.name
                    ));
                }
            }
        }
    }

    // ---- CAP: la particion no se elige, que hacer mientras dura si ----
    let lado: IndexMap<&str, &Cap> = ms.iter().map(|m| (m.service.as_str(), &m.cap)).collect();
    for m in ms.iter().filter(|m| !m.external) {
        let svc = &m.service;
        let cap = &m.cap;
        if !cap.declarado {
            warnings.push(format!(
                "{svc}: sin `[cap]`; asumido CP (strong/reject), que falla cerrado. \
                 Declaralo para que la eleccion sea de alguien y no del default"
            ));
        }
        match cap.consistency.as_str() {
            "strong" | "eventual" => {}
            otro => errors.push(format!(
                "{svc}: `consistency = \"{otro}\"` no existe; usa \"strong\" o \"eventual\""
            )),
        }
        match cap.on_partition.as_str() {
            "reject" | "degrade" => {}
            otro => errors.push(format!(
                "{svc}: `on_partition = \"{otro}\"` no existe; usa \"reject\" o \"degrade\""
            )),
        }
        // "eventual" sin un numero es una palabra, no una garantia
        if cap.eventual() && cap.max_staleness_ms.is_none() {
            errors.push(format!(
                "{svc}: `consistency = \"eventual\"` sin `max_staleness_ms`; sin un presupuesto \
                 de obsolescencia nadie puede decir si el dato que sirvio era aceptable"
            ));
        }
        // no se puede ser CP y servir algo viejo: es la contradiccion del teorema
        if !cap.eventual() && cap.degrada() {
            errors.push(format!(
                "{svc}: `strong` con `on_partition = \"degrade\"` se contradice; servir un dato \
                 viejo ES elegir disponibilidad sobre consistencia"
            ));
        }
        // tu garantia es la del eslabon mas debil de la ruta sincrona
        for d in &m.depends {
            if !cap.eventual() && lado.get(d.target()).is_some_and(|c| c.eventual()) {
                warnings.push(format!(
                    "{svc} es `strong` y llama a {} que es `eventual`: la garantia de la ruta \
                     es la del mas debil, no la tuya",
                    d.target()
                ));
            }
        }
        // decidir con consistencia fuerte a partir de una entrada que no la tiene
        for (nombre, mac) in &m.machine {
            for (act, t) in &mac.transitions {
                if !cap.eventual() && m.consumes.contains_key(&t.on) {
                    warnings.push(format!(
                        "{svc}.{nombre}.{act}: transicion `strong` disparada por el evento `{}`, \
                         que llega eventualmente; el estado puede haber cambiado antes",
                        t.on
                    ));
                }
            }
        }
    }

    // A08: fallos de integridad. Una etiqueta es mutable: lo que se despliega
    // hoy no es lo que se auditó ayer.
    if pol.ci.image.contains(":latest") || !pol.ci.image.contains('@') {
        warnings.push(format!(
            "[A08] [ci].image `{}` no fija un digest; una etiqueta es mutable y \
             el deploy deja de ser reproducible",
            pol.ci.image
        ));
    }

    // patrones de API: lo que separa un endpoint de uno que aguanta produccion
    let mut routes: IndexMap<String, String> = IndexMap::new();
    for m in ms.iter().filter(|m| !m.external) {
        for (name, meth) in &m.methods {
            let Some(http) = &meth.http else { continue };
            if let Some(prev) = routes.insert(http.clone(), m.service.clone()) {
                errors.push(format!("`{http}` declarada por {prev} y por {}", m.service));
            }
            match meth.path() {
                Some(p) if !p.starts_with("/v") => errors.push(format!(
                    "{}.{name}: `{p}` sin version en la ruta; usa /v1/...",
                    m.service
                )),
                _ => {}
            }
            if meth.mutating() && !meth.idempotent {
                errors.push(format!(
                    "{}.{name}: {http} muta sin `idempotent = true`; un reintento del cliente \
                     duplicaria el efecto",
                    m.service
                ));
            }
            // El gateway falla cerrado: una ruta expuesta sin decidir quien
            // puede llamarla no se despliega. No hay default seguro para esto.
            match meth.auth.as_deref() {
                Some("public") | Some("required") => {}
                Some(otro) => errors.push(format!(
                    "{}.{name}: `auth = \"{otro}\"` no existe; usa \"public\" o \"required\"",
                    m.service
                )),
                None => errors.push(format!(
                    "{}.{name}: {http} expuesta sin `auth`; declara \"public\" o \"required\"",
                    m.service
                )),
            }
            if meth.auth.as_deref() == Some("public") && meth.rate_limit.is_none() {
                errors.push(format!(
                    "{}.{name}: {http} es publica y sin `rate_limit`; el edge no tiene \
                     con que frenar un abuso",
                    m.service
                ));
            }
            if meth.paginated && !meth.output.contains_key("cursor") {
                errors.push(format!(
                    "{}.{name}: paginada pero no devuelve `cursor`; offset se rompe al crecer",
                    m.service
                ));
            }
        }
    }

    // maquinas de estado: estados muertos, inalcanzables y disparadores fantasma
    for m in ms.iter().filter(|m| !m.external) {
        for (name, mac) in &m.machine {
            let states = mac.states();
            if !states.contains(&mac.initial) {
                errors.push(format!(
                    "{}.{name}: estado inicial `{}` no aparece en ninguna transicion",
                    m.service, mac.initial
                ));
            }
            // alcanzabilidad desde el inicial
            let mut reach = vec![mac.initial.clone()];
            let mut grew = true;
            while grew {
                grew = false;
                for t in mac.transitions.values() {
                    if t.from.iter().any(|f| reach.contains(f)) && !reach.contains(&t.to) {
                        reach.push(t.to.clone());
                        grew = true;
                    }
                }
            }
            for st in &states {
                if !reach.contains(st) {
                    errors.push(format!(
                        "{}.{name}: estado `{st}` inalcanzable desde `{}`",
                        m.service, mac.initial
                    ));
                }
                let sale = mac.transitions.values().any(|t| t.from.contains(st));
                if !sale && !mac.final_states.contains(st) {
                    errors.push(format!(
                        "{}.{name}: `{st}` no es final y no tiene salida; es un deadlock",
                        m.service
                    ));
                }
            }
            for (act, t) in &mac.transitions {
                if !states.contains(&t.to) {
                    errors.push(format!(
                        "{}.{name}.{act}: destino `{}` desconocido",
                        m.service, t.to
                    ));
                }
                // el disparador tiene que existir de verdad
                if !m.methods.contains_key(&t.on) && !m.consumes.contains_key(&t.on) {
                    errors
                        .push(format!(
                        "{}.{name}.{act}: la dispara `{}`, que no es ni metodo ni evento consumido",
                        m.service, t.on));
                }
                if let Some(ev) = &t.emits {
                    if !m.emits.contains_key(ev) {
                        errors.push(format!(
                            "{}.{name}.{act}: emite `{ev}`, que el servicio no declara emitir",
                            m.service
                        ));
                    }
                }
                if let Some(c) = &t.compensates {
                    if !mac.transitions.contains_key(c) {
                        errors.push(format!(
                            "{}.{name}.{act}: compensa `{c}`, que no existe",
                            m.service
                        ));
                    }
                }
            }
        }
    }

    let known: IndexMap<&str, &Manifest> = ms.iter().map(|m| (m.service.as_str(), m)).collect();
    for m in ms {
        let svc = &m.service;
        for ev in m.consumes.keys() {
            if !emitters.contains_key(ev.as_str()) {
                errors.push(format!("{svc} consume {ev} pero nadie lo emite"));
            }
        }
        for d in &m.depends {
            let tgt = d.target();
            match known.get(tgt) {
                None => errors.push(format!("{svc} depende de {tgt}, sin manifiesto conocido")),
                Some(t) if !t.methods.contains_key(&d.method) => errors.push(format!(
                    "{svc} llama {tgt}.{}, que {tgt} no expone",
                    d.method
                )),
                _ => {}
            }
            if d.timeout_ms.is_none() {
                errors.push(format!(
                    "{svc} -> {tgt}.{}: sin `timeout_ms`; una llamada sin presupuesto de tiempo \
                     propaga la caida del otro lado",
                    d.method
                ));
            }
            if d.retries > 0
                && !known
                    .get(tgt)
                    .is_some_and(|t| t.methods.get(&d.method).is_some_and(|m| m.is_idempotent()))
            {
                errors.push(format!(
                    "{svc} reintenta {tgt}.{}, que no se declara idempotente",
                    d.method
                ));
            }
            if d.retries > 0 && !d.breaker {
                warnings.push(format!(
                    "{svc} -> {tgt}.{}: reintentos sin `breaker = true`; los reintentos amplifican \
                     la caida del otro lado", d.method));
            }
        }
    }

    // migraciones: expand -> migrate -> contract, y orden determinista
    for m in ms {
        for f in migrations_of(m) {
            let name = f
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            if destructive(&text, &f.display().to_string()) && !name.contains(".contract.") {
                errors.push(format!(
                    "{}/{name}: migracion destructiva sin marcar como `.contract.sql` \
                     (expand -> migrate -> contract)",
                    m.service
                ));
            }
            // `001_x.sql`: tres digitos y un guion bajo. Una regex para esto
            // seria una dependencia entera por tres caracteres.
            let numerado = {
                let b = name.as_bytes();
                b.len() > 3 && b[..3].iter().all(u8::is_ascii_digit) && b[3] == b'_'
            };
            if !numerado {
                warnings.push(format!(
                    "{}/{name}: sin prefijo numerico, el orden no es determinista",
                    m.service
                ));
            }
        }
    }

    // database per service: ninguna FK cruza el limite
    let by_svc = schemas(ms);
    let mut owner_of: IndexMap<&str, &str> = IndexMap::new();
    for (svc, tables) in &by_svc {
        for t in tables.keys() {
            owner_of.insert(t, svc);
        }
    }
    for (svc, tables) in &by_svc {
        for (t, cols) in tables {
            for c in cols {
                let Some(fk) = &c.fk else { continue };
                if let Some(owner) = owner_of.get(fk.as_str()) {
                    if owner != svc {
                        errors.push(format!(
                            "{svc}.{t}.{}: FK a {fk} cruza el limite de servicio \
                             (dueno: {owner}); guarda el id, no una FK",
                            c.name
                        ));
                    }
                }
            }
        }
    }

    for (ev, (owner, _)) in &emitters {
        if !ms.iter().any(|m| m.consumes.contains_key(*ev)) {
            warnings.push(format!("{ev} ({owner}) no tiene consumidores"));
        }
    }
    Report { errors, warnings }
}
