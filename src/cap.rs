//! Compatibility between the declared CAP side and the patterns in use.
//!
//! `verify` blocks the contradictions. This explains the consequences, which
//! is a different thing: there are combinations that are not an error and still
//! change what the service can promise. An outbox does not break the
//! consistency of your own state, but it does make consumers see it late —that
//! is not a bug, it is a property, and it is better written down somewhere
//! before somebody discovers it during an incident.
use crate::color::{blue, bold, green, grey, red, yellow};
use crate::manifest::*;

/// A finding's level, which decides the colour and the order.
enum Level {
    Contradicts,
    Costs,
    Implies,
}

struct Hallazgo {
    level: Level,
    pattern: String,
    text: String,
}

/// Pads to width BEFORE colouring: `{:<18}` counts the bytes of the ANSI
/// sequence, so padding already coloured text misaligns the columns.
fn column(t: &str, ancho: usize) -> String {
    bold(&format!("{t:<ancho$}"))
}

/// `only` filters by service name. The filtering happens at the end and not at
/// load time, because the analysis needs the others: without them there is no
/// way to know that a dependency is AP.
pub fn informe(ms: &[Manifest], only: &[String]) -> String {
    let lados: Vec<(&str, &Cap)> = ms.iter().map(|m| (m.service.as_str(), &m.cap)).collect();
    let mut o = Vec::new();

    for m in ms
        .iter()
        .filter(|m| !m.external)
        .filter(|m| only.is_empty() || only.contains(&m.service))
    {
        let cap = &m.cap;
        let lado = if cap.eventual() { "AP" } else { "CP" };
        o.push(format!(
            "\n{}  {}  {}",
            bold(&m.service),
            blue(&format!("[{lado}]")),
            grey(&format!(
                "consistency = {}, on_partition = {}{}",
                cap.consistency,
                cap.on_partition,
                cap.max_staleness_ms
                    .map(|v| format!(", max_staleness_ms = {v}"))
                    .unwrap_or_default()
            ))
        ));
        if !cap.declared {
            o.push(format!(
                "  {} the side is not declared: CP was assumed, which fails closed",
                yellow("~")
            ));
        }

        let mut hs: Vec<Hallazgo> = Vec::new();

        // --- outbox ---
        if m.patterns.outbox {
            hs.push(Hallazgo {
                level: Level::Implies,
                pattern: "outbox".into(),
                text: "your own state stays consistent, but consumers see it late: the \
                       relay publishes after the commit. That does not break your \
                       guarantee, it breaks the flow's"
                    .into(),
            });
        } else if !m.emits.is_empty() && !cap.eventual() {
            hs.push(Hallazgo {
                level: Level::Costs,
                pattern: "no outbox".into(),
                text: "CP is declared and publishing goes straight to the bus: if the commit \
                       lands and the publish fails, state and event disagree. That is the \
                       dual-write, \
                        y contra eso existe `[patterns] outbox`"
                    .into(),
            });
        }

        // --- read replicas ---
        match m.infra.read_replicas.unwrap_or(0) {
            0 => {}
            n if cap.eventual() => hs.push(Hallazgo {
                level: Level::Implies,
                pattern: "read replicas".into(),
                text: format!(
                    "{n} replicas consistent with AP: reads lag, and the budget is set by \
                     max_staleness_ms"
                ),
            }),
            n => hs.push(Hallazgo {
                level: Level::Contradicts,
                pattern: "read replicas".into(),
                text: format!(
                    "{n} replicas read under a CP promise: a replica lags"
                ),
            }),
        }

        // --- alta disponibilidad ---
        if m.infra.ha == Some(true) {
            hs.push(Hallazgo {
                level: Level::Implies,
                pattern: "standby HA".into(),
                text: "it breaks no consistency: nobody reads from the standby, it only takes \
                       over. It is the only thing on this list that improves \
                       availability at no cost in the C"
                    .into(),
            });
        } else if !cap.eventual() && m.infra.state.is_some() {
            hs.push(Hallazgo {
                level: Level::Costs,
                pattern: "no standby".into(),
                text: "CP with no failover: when the primary goes down, the service serves \
                       nothing. \
                        Consistente, si, y tambien apagado"
                    .into(),
            });
        }

        // --- sagas declaradas ---
        for (name, sg) in &m.saga {
            // `verify` already emits the error; here the cost is explained,
            // which is what this report adds.
            let compensated = sg.steps.iter().filter(|p| p.undo.is_some()).count();
            hs.push(Hallazgo {
                level: if cap.eventual() {
                    Level::Implies
                } else {
                    Level::Contradicts
                },
                pattern: format!("saga.{name}"),
                text: format!(
                    "{} steps, {compensated} with a compensation. Between the first step and \
                     the last there are visible intermediate states no invariant describes: \
                     your own state can be CP, the FLOW is eventual",
                    sg.steps.len()
                ),
            });
        }

        // --- sagas inside a state machine ---
        let compensa: Vec<&str> = m
            .machine
            .values()
            .flat_map(|mac| mac.transitions.iter())
            .filter(|(_, t)| t.compensates.is_some())
            .map(|(a, _)| a.as_str())
            .collect();
        if !compensa.is_empty() && !cap.eventual() {
            hs.push(Hallazgo {
                level: Level::Costs,
                pattern: "saga".into(),
                text: format!(
                    "`{}` compensates an earlier step. A compensation is eventual consistency \
                     by construction: your own state is CP, the FLOW is not",
                    compensa.join("`, `")
                ),
            });
        }

        // --- reintentos ---
        for d in &m.depends {
            if d.retries > 0 && !cap.eventual() && !d.breaker {
                hs.push(Hallazgo {
                    level: Level::Costs,
                    pattern: "retries".into(),
                    text: format!(
                        "`{}` is retried with no breaker under a CP promise: the retries \
                         lengthen the outage instead of shortening it",
                        d.method
                    ),
                });
            }
        }

        // --- the weakest link ---
        for d in &m.depends {
            let flojo = lados
                .iter()
                .find(|(s, _)| *s == d.target())
                .is_some_and(|(_, c)| c.eventual());
            if flojo && !cap.eventual() {
                hs.push(Hallazgo {
                    level: Level::Contradicts,
                    pattern: "dependency".into(),
                    text: format!(
                        "`{}`, which is AP, is called on a synchronous path: the path's \
                         guarantee is the weaker one",
                        d.target()
                    ),
                });
            }
        }

        // --- scaling to zero with a promise to reject ---
        if !cap.eventual() && !cap.degrades() && m.infra.min_instances == Some(0) {
            hs.push(Hallazgo {
                level: Level::Costs,
                pattern: "min_instances = 0".into(),
                text: "rejecting rather than degrading is promised, and it scales to zero: the \
                       first request after idling waits out a cold start with nothing to \
                        servir mientras tanto"
                    .into(),
            });
        }

        // --- degrading with no declared fallback ---
        if cap.degrades() && m.depends.is_empty() {
            hs.push(Hallazgo {
                level: Level::Implies,
                pattern: "degrade".into(),
                text: "there are no dependencies to degrade: the choice changes nothing yet"
                    .into(),
            });
        }

        if hs.is_empty() {
            o.push(format!("  {} nothing to reconcile", green("ok")));
        }
        hs.sort_by_key(|h| match h.level {
            Level::Contradicts => 0,
            Level::Costs => 1,
            Level::Implies => 2,
        });
        for h in hs {
            let (mark, label) = match h.level {
                Level::Contradicts => (red("x"), red("contradicts")),
                Level::Costs => (yellow("!"), yellow("costs")),
                Level::Implies => (blue("i"), blue("implies")),
            };
            o.push(format!(
                "  {mark} {} {label}  {}",
                column(&h.pattern, 18),
                h.text
            ));
        }
    }

    if o.is_empty() {
        return format!("{} no service by that name", yellow("warn"));
    }
    o.push(format!(
        "\n{}",
        grey(
            "x contradicts what is declared and `axon verify` blocks it · ! is a cost you \
             pay · i is a consequence worth knowing"
        )
    ));
    o.join("\n")
}
