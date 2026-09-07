//! The API's maintenance cycle, read out loud.
//!
//! `verify` blocks what is wrong. This explains what is TRUE and nobody can
//! see: which version is current, which ones are still supported, how many days
//! each one has left, and which methods changed shape at each step. Same
//! division as `axon cap` — it does not block, it tells.
use crate::color::{blue, bold, green, grey, red, yellow};
use crate::manifest::{date, epoch_days, today, Manifest};

pub fn report(ms: &[Manifest]) -> String {
    let Some(m) = ms.iter().find(|m| !m.external) else {
        return "no manifests\n".to_string();
    };
    let api = &m.api;
    let mut o = Vec::new();
    if !api.by_header() {
        // The path scheme has no cycle of its own: each version is a route, and
        // its retirement is declared on the method. Saying nothing here would
        // read as "there are no versions".
        o.push(format!(
            "{}  {}",
            bold("versioning = path"),
            grey("the version is the route; the retirement is declared per method")
        ));
        for m in ms.iter().filter(|m| !m.external) {
            for (name, me) in m.methods.iter().filter(|(_, me)| me.retiring()) {
                o.push(format!(
                    "  {} {}.{name}  {}",
                    yellow("~"),
                    m.service,
                    grey(&format!(
                        "{}{}{}",
                        me.http.as_deref().unwrap_or("-"),
                        me.sunset
                            .as_deref()
                            .map(|s| format!(" · sunset {s}"))
                            .unwrap_or_default(),
                        me.successor
                            .as_deref()
                            .map(|s| format!(" · successor {s}"))
                            .unwrap_or_default()
                    ))
                ));
            }
        }
        return format!("{}\n", o.join("\n"));
    }

    let now = today();
    let current = api.current().unwrap_or_default();
    o.push(format!(
        "{}  {}",
        bold(&format!("versioning = header ({})", api.header_name())),
        grey(&format!(
            "{} versions · default {current}{}",
            api.versions.len(),
            api.support_window_days
                .map(|d| format!(" · support window {d}d"))
                .unwrap_or_default()
        ))
    ));
    for v in api.versions.iter().rev() {
        let stage = v.stage(now, current);
        let paint = match stage {
            "current" => green(stage),
            "lts" => blue(stage),
            "supported" => grey(stage),
            "deprecated" => yellow(stage),
            _ => red(stage),
        };
        // Days left, which is the number nobody has and everybody needs to plan
        let left = v
            .sunset
            .as_deref()
            .and_then(date)
            .map(|s| epoch_days(s) - epoch_days(now))
            .map(|d| {
                if d < 0 {
                    "past its date".to_string()
                } else {
                    format!("{d}d left")
                }
            })
            .unwrap_or_else(|| "no death date".to_string());
        o.push(format!(
            "\n  {}  {}  {}",
            bold(&v.date),
            paint,
            grey(&format!(
                "{}{}",
                left,
                v.sunset
                    .as_deref()
                    .map(|s| format!(" · sunset {s}"))
                    .unwrap_or_default()
            ))
        ));
        // What changed at this version, which is what a caller has to migrate
        for other in ms.iter().filter(|m| !m.external) {
            for (name, me) in &other.methods {
                if let Some(shape) = me.at.get(&v.date) {
                    let changed: Vec<&str> = me
                        .output
                        .keys()
                        .filter(|k| shape.output.get(*k) != me.output.get(*k))
                        .map(|s| s.as_str())
                        .collect();
                    o.push(format!(
                        "    {} {}.{name}  {}",
                        grey("·"),
                        other.service,
                        grey(&format!(
                            "adapter {}{}",
                            shape.adapter.as_deref().unwrap_or("-"),
                            if changed.is_empty() {
                                String::new()
                            } else {
                                format!(" · changed: {}", changed.join(", "))
                            }
                        ))
                    ));
                }
            }
        }
    }
    format!("{}\n", o.join("\n"))
}
