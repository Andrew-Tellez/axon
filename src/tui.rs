//! The system as it is, drawn and animated.
//!
//! `verify` says whether it is coherent, `cap` what the guarantees cost,
//! `versions` where each version stands and `rules` what the loop proposes.
//! This shows the SHAPE: who talks to whom, over what, and what changed. A
//! drawing is not a rule, but the drawing is what nobody has, and a topology
//! that surprises you when you see it is usually one that was never decided.
//!
//! The layout is springs and repulsion —what talks together ends up together,
//! so a service pulled by six others sits in the middle looking like what it
//! is— and the drawing is ratatui. With `--frames N` it renders N frames to
//! stdout through ratatui's own test backend and exits: the picture is a
//! projection like any other, and a projection nobody verifies drifts.
use crate::manifest::{today, Manifest};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line as TextLine, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::time::Duration;
struct Node {
    name: String,
    side: String,
    external: bool,
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
}

/// An edge, and what kind of coupling it is: an event travels on its own and a
/// call waits for an answer. Drawing them the same would hide the only
/// difference that matters when the other side goes down.
struct Edge {
    from: usize,
    to: usize,
    label: String,
    async_: bool,
    warn: bool,
}

/// Nodes and edges from the manifests. An event goes from whoever emits it to
/// whoever consumes it —the direction of the fact, not of the dependency— and a
/// call goes the other way.
fn topology(ms: &[Manifest]) -> (Vec<Node>, Vec<Edge>) {
    let now = today();
    let mut nodes: Vec<Node> = ms
        .iter()
        .map(|m| Node {
            name: m.service.clone(),
            side: if m.external {
                "ext".into()
            } else if m.cap.eventual() {
                "AP".into()
            } else {
                "CP".into()
            },
            external: m.external,
            x: 0.0,
            y: 0.0,
            vx: 0.0,
            vy: 0.0,
        })
        .collect();
    // A deterministic starting circle: with a random one the same system draws
    // differently every run, and then no test can look at the picture.
    let n = nodes.len().max(1) as f64;
    for (i, node) in nodes.iter_mut().enumerate() {
        let a = i as f64 / n * std::f64::consts::TAU;
        node.x = a.cos() * 0.35;
        node.y = a.sin() * 0.35;
    }
    let idx = |name: &str| ms.iter().position(|m| m.service == name);
    let mut edges = Vec::new();
    for m in ms {
        for ev in m.emits.keys() {
            let Some(from) = idx(&m.service) else {
                continue;
            };
            for other in ms.iter().filter(|o| o.consumes.contains_key(ev)) {
                if let Some(to) = idx(&other.service) {
                    edges.push(Edge {
                        from,
                        to,
                        label: ev.clone(),
                        async_: true,
                        warn: false,
                    });
                }
            }
        }
        for d in &m.depends {
            let (Some(from), Some(to)) = (idx(&m.service), idx(d.target())) else {
                continue;
            };
            // A call at a version that is dying is the one thing on this drawing
            // that is going to break by itself, with nobody touching anything.
            let dying = ms
                .iter()
                .find(|o| o.service == d.target())
                .and_then(|o| o.methods.get(&d.method))
                .is_some_and(|me| me.retiring());
            edges.push(Edge {
                from,
                to,
                label: d.method.clone(),
                async_: false,
                warn: dying,
            });
        }
    }
    let _ = now;
    (nodes, edges)
}

/// One step of the layout: springs on the edges, repulsion between every pair,
/// a pull to the centre, and damping.
///
/// It is not decoration. A force-directed layout puts what talks together
/// together, so a service pulled by six others ends up in the middle of the
/// picture looking exactly like what it is.
fn step(nodes: &mut [Node], edges: &[Edge], dt: f64) {
    let n = nodes.len();
    for i in 0..n {
        let (mut fx, mut fy) = (0.0, 0.0);
        for j in 0..n {
            if i == j {
                continue;
            }
            let (dx, dy) = (nodes[i].x - nodes[j].x, nodes[i].y - nodes[j].y);
            let d2 = (dx * dx + dy * dy).max(0.004);
            let f = 0.02 / d2;
            let d = d2.sqrt();
            fx += dx / d * f;
            fy += dy / d * f;
        }
        // the centre, so nothing drifts off the screen
        fx -= nodes[i].x * 0.35;
        fy -= nodes[i].y * 0.35;
        nodes[i].vx += fx * dt;
        nodes[i].vy += fy * dt;
    }
    for e in edges {
        if e.from == e.to {
            continue;
        }
        let (dx, dy) = (
            nodes[e.to].x - nodes[e.from].x,
            nodes[e.to].y - nodes[e.from].y,
        );
        let d = (dx * dx + dy * dy).sqrt().max(0.02);
        // Hooke against a rest length: too short and the boxes overlap, too
        // long and the picture stops fitting
        let f = (d - 0.45) * 0.9;
        let (ux, uy) = (dx / d, dy / d);
        nodes[e.from].vx += ux * f * dt;
        nodes[e.from].vy += uy * f * dt;
        nodes[e.to].vx -= ux * f * dt;
        nodes[e.to].vy -= uy * f * dt;
    }
    for node in nodes.iter_mut() {
        node.vx *= 0.86;
        node.vy *= 0.86;
        node.x += node.vx * dt;
        node.y += node.vy * dt;
    }
}

/// The state that is not shape: the verdict, the versions and what changed.
struct Status {
    verdict: String,
    verdict_color: Color,
    errors: Vec<String>,
    warnings: Vec<String>,
    versions: String,
    changed: String,
    /// One line per service: its topics and what it runs on. The drawing says
    /// who talks to whom; a topic nobody consumes yet, or a database, is state
    /// that no edge can show.
    services: Vec<String>,
}

fn status(ms: &[Manifest], root: &std::path::Path) -> Status {
    let report = crate::verify::verify(ms, &crate::verify::load_policy(root));
    let (verdict, verdict_color) = if !report.errors.is_empty() {
        (
            format!(
                "fail · {} errors {} warn",
                report.errors.len(),
                report.warnings.len()
            ),
            Color::Red,
        )
    } else if !report.warnings.is_empty() {
        (
            format!("near · 0 errors {} warn", report.warnings.len()),
            Color::Yellow,
        )
    } else {
        ("ok · nothing to fix".to_string(), Color::Green)
    };

    // The versions, as a line: what is current and what is still alive.
    let api = ms.iter().find(|m| !m.external).map(|m| &m.api);
    let now = today();
    let versions = match api.filter(|a| a.by_header()) {
        Some(a) => {
            let current = a.current().unwrap_or_default();
            a.versions
                .iter()
                .rev()
                .map(|v| format!("{} {}", v.date, v.stage(now, current)))
                .collect::<Vec<_>>()
                .join(" · ")
        }
        None => {
            let retiring: Vec<String> = ms
                .iter()
                .filter(|m| !m.external)
                .flat_map(|m| m.methods.iter())
                .filter(|(_, me)| me.retiring())
                .map(|(name, me)| {
                    format!(
                        "{name} deprecated{}",
                        me.sunset
                            .as_deref()
                            .map(|s| format!(" · sunset {s}"))
                            .unwrap_or_default()
                    )
                })
                .collect();
            if retiring.is_empty() {
                "path · nothing retiring".to_string()
            } else {
                format!("path · {}", retiring.join(" · "))
            }
        }
    };

    // What changed: against the baseline, which is the record of what was
    // published. It is the only honest answer to "how is the system now"
    // — the rest is how it was written down.
    let changed = match crate::baseline::cargar(root) {
        None => "no baseline: nothing to compare against".to_string(),
        Some(b) => {
            let now_ = crate::baseline::tomar(ms);
            let mut bits = Vec::new();
            let new_events = now_
                .events
                .keys()
                .filter(|k| !b.events.contains_key(*k))
                .count();
            let new_methods = now_
                .methods
                .keys()
                .filter(|k| !b.methods.contains_key(*k))
                .count();
            if new_events > 0 {
                bits.push(format!("+{new_events} events"));
            }
            if new_methods > 0 {
                bits.push(format!("+{new_methods} methods"));
            }
            let (errs, _) = crate::baseline::comparar(ms, &b);
            if !errs.is_empty() {
                bits.push(format!("{} breaking", errs.len()));
            }
            if bits.is_empty() {
                "nothing new since the last baseline".to_string()
            } else {
                bits.join("  ")
            }
        }
    };
    // What each service declares, as a line. A single-service project draws a
    // graph with no edges, and then the picture alone says almost nothing.
    let services = ms
        .iter()
        .map(|m| {
            let mut bits = Vec::new();
            if !m.emits.is_empty() {
                bits.push(format!("emits {}", keys(m.emits.keys())));
            }
            if !m.consumes.is_empty() {
                bits.push(format!("consumes {}", keys(m.consumes.keys())));
            }
            if !m.methods.is_empty() {
                bits.push(format!("{} methods", m.methods.len()));
            }
            if let Some(state) = m.infra.state.as_deref() {
                bits.push(state.to_string());
            }
            if let Some(e) = m.cache.engine.as_deref().filter(|_| m.cache.active()) {
                bits.push(e.to_string());
            }
            if let Some(e) = m.search.engine.as_deref().filter(|_| m.search.active()) {
                bits.push(e.to_string());
            }
            if bits.is_empty() {
                bits.push("nothing declared".into());
            }
            format!("{}  {}", m.service, bits.join("  ·  "))
        })
        .collect();
    Status {
        verdict,
        verdict_color,
        errors: report.errors,
        warnings: report.warnings,
        versions,
        changed,
        services,
    }
}

/// `a b c` — the topic names as they are declared, so what the panel shows is
/// what somebody would grep the manifest for.
fn keys<'a>(k: impl Iterator<Item = &'a String>) -> String {
    k.cloned().collect::<Vec<_>>().join(" ")
}

/// Everything one frame needs. Kept apart from the drawing so `--frames` and
/// the live loop render exactly the same thing.
struct App {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    status: Status,
    panel: usize,
    frame: u64,
    paused: bool,
    /// First visible line of the panel, and how far it can go. The ceiling
    /// depends on the wrapped height, which only the drawing knows, so it is
    /// written there and read by the key that moves the scroll.
    scroll: usize,
    max_scroll: std::cell::Cell<usize>,
}

impl App {
    fn new(ms: &[Manifest], root: &std::path::Path) -> Self {
        let (mut nodes, edges) = topology(ms);
        // Settle before the first frame: the layout starts on a circle, and
        // watching it unfold from there is pretty exactly once. What is worth
        // seeing is where it ENDS UP, so it opens already relaxed and keeps
        // breathing from there.
        for _ in 0..120 {
            step(&mut nodes, &edges, 0.08);
        }
        Self {
            nodes,
            edges,
            status: status(ms, root),
            panel: 0,
            frame: 0,
            paused: false,
            scroll: 0,
            max_scroll: std::cell::Cell::new(0),
        }
    }
    fn tick(&mut self) {
        if !self.paused {
            step(&mut self.nodes, &self.edges, 0.08);
            self.frame += 1;
        }
    }
}

/// The graph, on a braille canvas: the edges are lines and the nodes are
/// printed labels. An event and a call are drawn differently on purpose —
/// one travels on its own and the other waits for an answer, and that is the
/// difference that matters when the other side goes down.
fn draw_graph(f: &mut Frame, area: Rect, app: &App) {
    let edges = &app.edges;
    let frame = app.frame;
    // The picture is scaled to what there IS, not to what the constants of the
    // simulation give: with three services or with thirty, the graph fills the
    // canvas instead of sitting in a knot in the middle.
    let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for n in &app.nodes {
        lo_x = lo_x.min(n.x);
        hi_x = hi_x.max(n.x);
        lo_y = lo_y.min(n.y);
        hi_y = hi_y.max(n.y);
    }
    // An axis can collapse —one service, or several in a row— and then every
    // node maps to the low edge and the picture ends up parked in a corner
    // instead of where it is. A collapsed axis is centred.
    let fit = |v: f64, lo: f64, span: f64, scale: f64| {
        if span < 1e-6 {
            0.0
        } else {
            (v - lo) / span * scale - scale / 2.0
        }
    };
    let span_x = hi_x - lo_x;
    let span_y = hi_y - lo_y;
    let nodes: Vec<Node> = app
        .nodes
        .iter()
        .map(|n| Node {
            name: n.name.clone(),
            side: n.side.clone(),
            external: n.external,
            x: fit(n.x, lo_x, span_x, 1.7),
            y: fit(n.y, lo_y, span_y, 1.5),
            vx: 0.0,
            vy: 0.0,
        })
        .collect();
    let nodes = &nodes;
    let canvas = Canvas::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(Span::styled(
                    " axon ",
                    Style::default().add_modifier(Modifier::BOLD),
                ))
                .title_top(
                    TextLine::from(Span::styled(
                        format!(" {} ", app.status.verdict),
                        Style::default().fg(app.status.verdict_color),
                    ))
                    .right_aligned(),
                ),
        )
        .marker(Marker::Braille)
        .x_bounds([-1.15, 1.15])
        .y_bounds([-1.15, 1.15])
        .paint(move |ctx| {
            for e in edges {
                if e.from == e.to {
                    continue;
                }
                let (a, b) = (&nodes[e.from], &nodes[e.to]);
                ctx.draw(&CanvasLine {
                    x1: a.x,
                    y1: a.y,
                    x2: b.x,
                    y2: b.y,
                    color: if e.warn {
                        Color::Yellow
                    } else if e.async_ {
                        Color::Blue
                    } else {
                        Color::DarkGray
                    },
                });
            }
            ctx.layer();
            for e in edges {
                if e.from == e.to {
                    continue;
                }
                let (a, b) = (&nodes[e.from], &nodes[e.to]);
                // The pulse: where the message is on this edge right now. Only
                // the async ones carry it — a call is not a thing travelling,
                // it is somebody waiting.
                if e.async_ {
                    let t =
                        ((frame as f64 * 0.02) + e.from as f64 * 0.17 + e.to as f64 * 0.31).fract();
                    ctx.print(
                        a.x + (b.x - a.x) * t,
                        a.y + (b.y - a.y) * t,
                        Span::styled("●", Style::default().fg(Color::Green)),
                    );
                }
                // the label at the middle, and only for the ones that are dying
                if e.warn {
                    ctx.print(
                        (a.x + b.x) / 2.0,
                        (a.y + b.y) / 2.0,
                        Span::styled(format!("{} ⚠", e.label), Style::default().fg(Color::Yellow)),
                    );
                }
            }
            ctx.layer();
            for n in nodes {
                let style = if n.external {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                };
                // Clamped so the name fits: printed from its centre, a long
                // one at the edge falls off the canvas and the node disappears
                // from the picture without anything saying so.
                let half = 0.028 * (n.name.chars().count() + 5) as f64;
                ctx.print(
                    (n.x - half).clamp(-1.12, 1.12 - 2.0 * half),
                    n.y,
                    TextLine::from(vec![
                        Span::styled(format!(" {} ", n.name), style),
                        Span::styled(
                            format!("[{}]", n.side),
                            Style::default().fg(if n.external {
                                Color::DarkGray
                            } else if n.side == "CP" {
                                Color::Blue
                            } else {
                                Color::Yellow
                            }),
                        ),
                    ]),
                );
            }
        });
    f.render_widget(canvas, area);
}

/// How tall the text is once wrapped. `Paragraph` knows, but only behind an
/// unstable feature, and the arithmetic is the same: a line takes as many rows
/// as it has widths, and an empty one still takes one.
fn wrapped_height(lines: &[String], width: usize) -> usize {
    lines
        .iter()
        .map(|l| l.chars().count().div_ceil(width.max(1)).max(1))
        .sum()
}

/// How many panels Tab cycles through. It is a constant so the key that
/// advances it and the array it indexes cannot disagree.
const PANELS: usize = 4;

/// The panels: the drawing answers "what shape is it" and these answer "and
/// how is it". Tab cycles them, because three lines of state is what fits and
/// hiding the rest behind a key beats truncating all of it.
fn draw_panels(f: &mut Frame, area: Rect, app: &App) {
    let panels: [(&str, Vec<String>); PANELS] = [
        ("services", app.status.services.clone()),
        (
            "state",
            vec![
                format!("versions  {}", app.status.versions),
                format!("changed   {}", app.status.changed),
            ],
        ),
        (
            "errors",
            if app.status.errors.is_empty() {
                vec!["no errors: everything declared agrees".into()]
            } else {
                app.status.errors.clone()
            },
        ),
        (
            "warnings",
            if app.status.warnings.is_empty() {
                vec!["no warnings".into()]
            } else {
                app.status.warnings.clone()
            },
        ),
    ];
    let (name, lines) = &panels[app.panel];
    let body: Vec<TextLine> = lines
        .iter()
        .map(|l| TextLine::from(Span::styled(l.clone(), Style::default().fg(Color::Gray))))
        .collect();
    // The inside of the block, which is what the text has to fit in: the
    // borders take a column and a row on each side.
    let inner_w = area.width.saturating_sub(2).max(1) as usize;
    let inner_h = area.height.saturating_sub(2).max(1) as usize;
    let max_scroll = wrapped_height(lines, inner_w).saturating_sub(inner_h);
    app.max_scroll.set(max_scroll);
    let scroll = app.scroll.min(max_scroll);
    let keys = if app.paused {
        "[tab] panel  [space] run  [r] re-read  [q] quit"
    } else {
        "[tab] panel  [space] pause  [r] re-read  [q] quit"
    };
    // The scroll is only mentioned when there IS something below: a hint for a
    // key that does nothing is worse than no hint.
    let more = if max_scroll > 0 {
        format!(" {}/{} ↑↓ ", scroll + 1, max_scroll + 1)
    } else {
        String::new()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" {name} "),
            Style::default().add_modifier(Modifier::BOLD),
        ))
        .title_top(
            TextLine::from(Span::styled(more, Style::default().fg(Color::DarkGray)))
                .right_aligned(),
        )
        .title_bottom(
            TextLine::from(Span::styled(
                format!(" {keys}  ·  frame {} ", app.frame),
                Style::default().fg(Color::DarkGray),
            ))
            .right_aligned(),
        );
    f.render_widget(
        Paragraph::new(body)
            .block(block)
            // Wrapped and not clipped: a truncated line hides what it says
            // without saying that it is hiding it. `trim: false` keeps the
            // indentation of a wrapped continuation.
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0)),
        area,
    );
}

fn draw(f: &mut Frame, app: &App) {
    let [top, bottom] =
        Layout::vertical([Constraint::Min(8), Constraint::Length(6)]).areas(f.area());
    draw_graph(f, top, app);
    draw_panels(f, bottom, app);
}

/// `--frames N`: N frames through ratatui's own test backend, as text. It is
/// what makes the picture checkable in CI and what makes it usable in a pipe.
pub fn frames(ms: &[Manifest], root: &std::path::Path, n: u64) -> Result<String, String> {
    let mut app = App::new(ms, root);
    let backend = ratatui::backend::TestBackend::new(78, 24);
    let mut term = Terminal::new(backend).map_err(|e| e.to_string())?;
    let mut out = String::new();
    for _ in 0..n.max(1) {
        // one step per frame, the same as the live loop, so what a test looks
        // at is what a person sees
        app.tick();
        term.draw(|f| draw(f, &app)).map_err(|e| e.to_string())?;
        let buffer = term.backend().buffer();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
    }
    Ok(out)
}

/// The live loop. `ratatui::init` takes the terminal and `restore` gives it
/// back, including on a panic: a tool that leaves the terminal unusable is
/// worse than one that does not draw.
pub fn run(ms: &[Manifest], root: &std::path::Path) -> Result<(), String> {
    let mut app = App::new(ms, root);
    let mut term = ratatui::init();
    let result = (|| -> Result<(), String> {
        loop {
            term.draw(|f| draw(f, &app)).map_err(|e| e.to_string())?;
            // The poll is the clock: it draws at ~30fps and a key is read the
            // moment it arrives, without a thread and without a busy loop.
            if event::poll(Duration::from_millis(33)).map_err(|e| e.to_string())? {
                if let Event::Key(k) = event::read().map_err(|e| e.to_string())? {
                    if k.kind == KeyEventKind::Press {
                        match k.code {
                            KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                            KeyCode::Tab => {
                                app.panel = (app.panel + 1) % PANELS;
                                // Another panel is another text: keeping the
                                // offset lands you in the middle of it.
                                app.scroll = 0;
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                app.scroll = (app.scroll + 1).min(app.max_scroll.get())
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                app.scroll = app.scroll.saturating_sub(1)
                            }
                            KeyCode::Char(' ') => app.paused = !app.paused,
                            // Re-reading is deliberate and not on a timer: a
                            // picture that changes under you while you look at
                            // it cannot be read.
                            KeyCode::Char('r') => app.status = status(ms, root),
                            _ => {}
                        }
                    }
                }
            }
            app.tick();
        }
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::wrapped_height;

    /// The scroll ceiling comes out of this: get it wrong and the panel either
    /// hides its last line forever or scrolls past the end into blank rows.
    #[test]
    fn the_wrapped_height_counts_the_rows_a_panel_takes() {
        let lines = vec!["abcdef".to_string(), String::new(), "abcd".to_string()];
        // 6 chars over 4 columns is two rows, the empty line still takes one,
        // and an exact fit takes exactly one
        assert_eq!(wrapped_height(&lines, 4), 4);
        assert_eq!(wrapped_height(&lines, 6), 3);
        // a zero width would divide by zero instead of saying so
        assert_eq!(wrapped_height(&["ab".to_string()], 0), 2);
    }
}
