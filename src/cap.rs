//! Compatibilidad entre el lado CAP declarado y los patrones en uso.
//!
//! `verify` bloquea las contradicciones. Esto explica las consecuencias, que
//! es distinto: hay combinaciones que no son un error y aun asi cambian lo que
//! el servicio puede prometer. Un outbox no rompe la consistencia del estado
//! propio, pero si hace que los consumidores la vean tarde — eso no es un bug,
//! es una propiedad, y conviene que este escrita en alguna parte antes de que
//! alguien la descubra en un incidente.
use crate::color::{amarillo, azul, fuerte, gris, rojo, verde};
use crate::manifest::*;

/// Nivel de un hallazgo, que decide el color y el orden.
enum Nivel {
    Contradice,
    Cuesta,
    Implica,
}

struct Hallazgo {
    nivel: Nivel,
    patron: String,
    texto: String,
}

/// Rellena a lo ancho ANTES de colorear: `{:<18}` cuenta los bytes de la
/// secuencia ANSI, asi que padear texto ya coloreado desalinea las columnas.
fn columna(t: &str, ancho: usize) -> String {
    fuerte(&format!("{t:<ancho$}"))
}

/// `solo` filtra por nombre de servicio. Se filtra al final y no al cargar,
/// porque el analisis necesita a los demas: sin ellos no se puede saber que
/// una dependencia es AP.
pub fn informe(ms: &[Manifest], solo: &[String]) -> String {
    let lados: Vec<(&str, &Cap)> = ms.iter().map(|m| (m.service.as_str(), &m.cap)).collect();
    let mut o = Vec::new();

    for m in ms
        .iter()
        .filter(|m| !m.external)
        .filter(|m| solo.is_empty() || solo.contains(&m.service))
    {
        let cap = &m.cap;
        let lado = if cap.eventual() { "AP" } else { "CP" };
        o.push(format!(
            "\n{}  {}  {}",
            fuerte(&m.service),
            azul(&format!("[{lado}]")),
            gris(&format!(
                "consistency = {}, on_partition = {}{}",
                cap.consistency,
                cap.on_partition,
                cap.max_staleness_ms
                    .map(|v| format!(", max_staleness_ms = {v}"))
                    .unwrap_or_default()
            ))
        ));
        if !cap.declarado {
            o.push(format!(
                "  {} el lado no esta declarado: se asumio CP, que falla cerrado",
                amarillo("~")
            ));
        }

        let mut hs: Vec<Hallazgo> = Vec::new();

        // --- outbox ---
        if m.patterns.outbox {
            hs.push(Hallazgo {
                nivel: Nivel::Implica,
                patron: "outbox".into(),
                texto: "el estado propio queda consistente, pero los consumidores lo ven \
                        tarde: el relay publica despues de confirmar. Eso no rompe tu \
                        garantia, rompe la del flujo"
                    .into(),
            });
        } else if !m.emits.is_empty() && !cap.eventual() {
            hs.push(Hallazgo {
                nivel: Nivel::Cuesta,
                patron: "sin outbox".into(),
                texto: "se declara CP y se publica directo al bus: si el commit sale y la \
                        publicacion falla, el estado y el evento discrepan. Es el dual-write, \
                        y contra eso existe `[patterns] outbox`"
                    .into(),
            });
        }

        // --- replicas de lectura ---
        match m.infra.read_replicas.unwrap_or(0) {
            0 => {}
            n if cap.eventual() => hs.push(Hallazgo {
                nivel: Nivel::Implica,
                patron: "read replicas".into(),
                texto: format!(
                    "{n} replicas coherentes con AP: se lee con retraso, y el presupuesto \
                     lo fija max_staleness_ms"
                ),
            }),
            n => hs.push(Hallazgo {
                nivel: Nivel::Contradice,
                patron: "read replicas".into(),
                texto: format!(
                    "{n} replicas leidas bajo una promesa CP: una replica va con retraso"
                ),
            }),
        }

        // --- alta disponibilidad ---
        if m.infra.ha == Some(true) {
            hs.push(Hallazgo {
                nivel: Nivel::Implica,
                patron: "standby HA".into(),
                texto: "no rompe la consistencia: del standby no se lee, solo toma el relevo. \
                        Es lo unico de esta lista que mejora la disponibilidad sin costo en la C"
                    .into(),
            });
        } else if !cap.eventual() && m.infra.state.is_some() {
            hs.push(Hallazgo {
                nivel: Nivel::Cuesta,
                patron: "sin standby".into(),
                texto: "CP sin failover: cuando el primario se cae, el servicio no sirve nada. \
                        Consistente, si, y tambien apagado"
                    .into(),
            });
        }

        // --- sagas declaradas ---
        for (nombre, sg) in &m.saga {
            // El error ya lo emite `verify`; aca se explica el costo, que es
            // lo que este informe agrega.
            let compensables = sg.steps.iter().filter(|p| p.undo.is_some()).count();
            hs.push(Hallazgo {
                nivel: if cap.eventual() {
                    Nivel::Implica
                } else {
                    Nivel::Contradice
                },
                patron: format!("saga.{nombre}"),
                texto: format!(
                    "{} pasos, {compensables} con compensacion. Entre el primer paso y el \
                     ultimo hay estados intermedios visibles que ningun invariante describe: \
                     el estado propio puede ser CP, el FLUJO es eventual",
                    sg.steps.len()
                ),
            });
        }

        // --- sagas dentro de una maquina de estado ---
        let compensa: Vec<&str> = m
            .machine
            .values()
            .flat_map(|mac| mac.transitions.iter())
            .filter(|(_, t)| t.compensates.is_some())
            .map(|(a, _)| a.as_str())
            .collect();
        if !compensa.is_empty() && !cap.eventual() {
            hs.push(Hallazgo {
                nivel: Nivel::Cuesta,
                patron: "saga".into(),
                texto: format!(
                    "`{}` compensa un paso anterior. Una compensacion es consistencia \
                     eventual por construccion: el estado propio es CP, el FLUJO no",
                    compensa.join("`, `")
                ),
            });
        }

        // --- reintentos ---
        for d in &m.depends {
            if d.retries > 0 && !cap.eventual() && !d.breaker {
                hs.push(Hallazgo {
                    nivel: Nivel::Cuesta,
                    patron: "reintentos".into(),
                    texto: format!(
                        "se reintenta `{}` sin breaker bajo una promesa CP: los reintentos \
                         alargan la indisponibilidad en vez de acortarla",
                        d.method
                    ),
                });
            }
        }

        // --- el eslabon mas debil ---
        for d in &m.depends {
            let flojo = lados
                .iter()
                .find(|(s, _)| *s == d.target())
                .is_some_and(|(_, c)| c.eventual());
            if flojo && !cap.eventual() {
                hs.push(Hallazgo {
                    nivel: Nivel::Contradice,
                    patron: "dependencia".into(),
                    texto: format!(
                        "se llama a `{}`, que es AP, en una ruta sincrona: la garantia de la \
                         ruta es la del mas debil",
                        d.target()
                    ),
                });
            }
        }

        // --- escalar a cero con una promesa de rechazar ---
        if !cap.eventual() && !cap.degrada() && m.infra.min_instances == Some(0) {
            hs.push(Hallazgo {
                nivel: Nivel::Cuesta,
                patron: "min_instances = 0".into(),
                texto: "se promete rechazar antes que degradar, y se escala a cero: la primera \
                        peticion despues del reposo espera un arranque en frio sin nada que \
                        servir mientras tanto"
                    .into(),
            });
        }

        // --- degradar sin respaldo declarado ---
        if cap.degrada() && m.depends.is_empty() {
            hs.push(Hallazgo {
                nivel: Nivel::Implica,
                patron: "degrade".into(),
                texto: "no hay dependencias que degradar: la eleccion no cambia nada todavia"
                    .into(),
            });
        }

        if hs.is_empty() {
            o.push(format!("  {} nada que reconciliar", verde("ok")));
        }
        hs.sort_by_key(|h| match h.nivel {
            Nivel::Contradice => 0,
            Nivel::Cuesta => 1,
            Nivel::Implica => 2,
        });
        for h in hs {
            let (marca, etiqueta) = match h.nivel {
                Nivel::Contradice => (rojo("x"), rojo("contradice")),
                Nivel::Cuesta => (amarillo("!"), amarillo("cuesta")),
                Nivel::Implica => (azul("i"), azul("implica")),
            };
            o.push(format!(
                "  {marca} {} {etiqueta}  {}",
                columna(&h.patron, 18),
                h.texto
            ));
        }
    }

    if o.is_empty() {
        return format!("{} ningun servicio con ese nombre", amarillo("aviso"));
    }
    o.push(format!(
        "\n{}",
        gris(
            "x contradice lo declarado y `axon verify` lo bloquea · ! es un costo que se paga \
             · i es una consecuencia que conviene conocer"
        )
    ));
    o.join("\n")
}
