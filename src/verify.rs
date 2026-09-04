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
