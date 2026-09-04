//! Baseline: los contratos ya publicados.
//!
//! Sin esto, `verify` solo detecta que DOS servicios declaren el mismo evento
//! distinto. No detecta lo mas comun y lo mas caro: cambiarle un campo a una
//! version que ya esta en produccion, con consumidores desplegados que la
//! esperan como estaba. Una version publicada es inmutable, y eso solo se
//! puede comprobar contra un registro de lo que se publico.
use crate::manifest::*;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const ARCHIVO: &str = "axon.baseline.json";

#[derive(Debug, Serialize, Deserialize)]
pub struct Evento {
    pub owner: String,
    pub fields: Fields,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Firma {
    #[serde(rename = "in")]
    pub input: Fields,
    #[serde(rename = "out")]
    pub output: Fields,
    pub http: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Baseline {
    /// Nota para quien abra el archivo sin contexto.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub nota: String,
    #[serde(default)]
    pub events: IndexMap<String, Evento>,
    /// Clave `servicio.metodo`.
    #[serde(default)]
    pub methods: IndexMap<String, Firma>,
}

pub fn tomar(ms: &[Manifest]) -> Baseline {
    let mut b = Baseline {
        nota: "Contratos publicados. Generado por `axon baseline`. Un cambio aqui es \
               un cambio incompatible: se revisa en el PR como cualquier otro."
            .into(),
        ..Default::default()
    };
    for m in ms.iter().filter(|m| !m.external) {
        for (ev, fields) in &m.emits {
            b.events.insert(
                ev.clone(),
                Evento {
                    owner: m.service.clone(),
                    fields: fields.clone(),
                },
            );
        }
        for (name, me) in &m.methods {
            b.methods.insert(
                format!("{}.{name}", m.service),
                Firma {
                    input: me.input.clone(),
                    output: me.output.clone(),
                    http: me.http.clone(),
                },
            );
        }
    }
    b.events.sort_keys();
    b.methods.sort_keys();
    b
}

pub fn cargar(dir: &Path) -> Option<Baseline> {
    let f = dir.join(ARCHIVO);
    let text = std::fs::read_to_string(&f).ok()?;
    match serde_json::from_str(&text) {
        Ok(b) => Some(b),
        Err(e) => {
            eprintln!("axon: {}: baseline invalido: {e}", f.display());
            std::process::exit(1);
        }
    }
}

/// Compara lo declarado contra lo publicado. Solo mira hacia atras: un evento
/// o metodo nuevo no es un problema, cambiar uno viejo si.
pub fn comparar(ms: &[Manifest], b: &Baseline) -> (Vec<String>, Vec<String>) {
    let mut errores = Vec::new();
    let mut avisos = Vec::new();
    let ahora = tomar(ms);

    // Un contrato que no esta registrado no esta protegido: manana se le puede
    // cambiar un campo y nadie lo va a notar.
    let nuevos: Vec<&String> = ahora
        .events
        .keys()
        .filter(|k| !b.events.contains_key(*k))
        .chain(ahora.methods.keys().filter(|k| !b.methods.contains_key(*k)))
        .collect();
    if !nuevos.is_empty() {
        avisos.push(format!(
            "{} contratos sin registrar en {ARCHIVO} ({}); corre `axon baseline` al publicarlos",
            nuevos.len(),
            nuevos
                .iter()
                .take(3)
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    for (ev, antes) in &b.events {
        match ahora.events.get(ev) {
            None => errores.push(format!(
                "{ev}: estaba publicado por {} y ya nadie lo emite; sus consumidores \
                 siguen desplegados. Si de verdad se retira, quitalo del {ARCHIVO} en el mismo PR",
                antes.owner
            )),
            Some(hoy) if hoy.owner != antes.owner => errores.push(format!(
                "{ev}: cambio de dueno, de {} a {}; un evento tiene un solo dueno",
                antes.owner, hoy.owner
            )),
            Some(hoy) if hoy.fields != antes.fields => {
                for (campo, tipo) in &antes.fields {
                    match hoy.fields.get(campo) {
                        None => errores.push(format!(
                            "{ev}: desaparecio el campo `{campo}` de una version publicada; \
                             publica {} en su lugar",
                            siguiente(ev)
                        )),
                        Some(t) if t != tipo => errores.push(format!(
                            "{ev}.{campo}: cambio de `{tipo}` a `{t}` en una version publicada; \
                             publica {} en su lugar",
                            siguiente(ev)
                        )),
                        _ => {}
                    }
                }
                for campo in hoy.fields.keys() {
                    if !antes.fields.contains_key(campo) {
                        // Todo campo de axon es obligatorio: agregar uno rompe
                        // a los productores viejos igual que quitarlo rompe a
                        // los consumidores.
                        errores.push(format!(
                            "{ev}: campo nuevo `{campo}` en una version publicada; \
                             publica {} en su lugar",
                            siguiente(ev)
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    for (clave, antes) in &b.methods {
        match ahora.methods.get(clave) {
            None => errores.push(format!(
                "{clave}: estaba publicado y ya no existe; sus llamadores siguen desplegados"
            )),
            Some(hoy) => {
                if hoy.http != antes.http {
                    let r = |o: &Option<String>| o.clone().unwrap_or_else(|| "(sin ruta)".into());
                    errores.push(format!(
                        "{clave}: la ruta cambio de `{}` a `{}`; los clientes apuntan a la vieja",
                        r(&antes.http),
                        r(&hoy.http)
                    ));
                }
                for (campo, tipo) in &antes.output {
                    match hoy.output.get(campo) {
                        None => errores.push(format!(
                            "{clave}: dejo de devolver `{campo}`; los llamadores lo leen"
                        )),
                        Some(t) if t != tipo => errores.push(format!(
                            "{clave}: `{campo}` de salida cambio de `{tipo}` a `{t}`"
                        )),
                        _ => {}
                    }
                }
                for (campo, tipo) in &hoy.input {
                    match antes.input.get(campo) {
                        None => errores.push(format!(
                            "{clave}: entrada nueva `{campo}`, obligatoria; los llamadores \
                             viejos no la mandan"
                        )),
                        Some(antes_tipo) if antes_tipo != tipo => errores.push(format!(
                            "{clave}: `{campo}` de entrada cambio de `{antes_tipo}` a `{tipo}`"
                        )),
                        _ => {}
                    }
                }
            }
        }
    }
    (errores, avisos)
}

/// `order.placed@v1` -> `order.placed@v2`
fn siguiente(ev: &str) -> String {
    match ev.rsplit_once("@v") {
        Some((base, n)) => match n.parse::<u32>() {
            Ok(v) => format!("`{base}@v{}`", v + 1),
            Err(_) => format!("`{ev}` con una version nueva"),
        },
        None => format!("`{ev}` con una version nueva"),
    }
}
