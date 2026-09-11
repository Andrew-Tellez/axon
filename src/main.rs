//! axon — the manifest is the source of truth; the rest are projections.
mod accepted;
mod api;
mod baseline;
mod bi;
mod cap;
mod carga;
mod catalog;
mod color;
mod dbsec;
mod emit;
mod gen_go;
mod import;
mod infra;
mod init;
mod lsp;
mod manifest;
mod pact;
mod plugin;
mod pooler;
mod trace;
mod traffic;
mod tui;
mod verify;
mod versions;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "axon",
    version,
    about = "Manifest-first compiler for event-driven microservices"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// manifest -> contracts and base class
    Build {
        /// a path, or the URL of a service that serves its own manifest
        manifest: String,
        /// The other manifests: that is where the type of what this service consumes comes from.
        sources: Vec<String>,
        #[arg(long, default_value = "ts")]
        lang: String,
    },
    /// manifest -> CI/CD pipeline
    Ci {
        manifest: PathBuf,
        /// deploy platform; without it only the gates get generated
        #[arg(long, default_value = "none")]
        target: String,
        /// forge that runs the pipeline
        #[arg(long, default_value = "github", value_parser = emit::FORGES)]
        forge: String,
    },
    /// manifests -> IaC. `--target plan` gives the neutral plan in JSON.
    Infra {
        sources: Vec<String>,
        #[arg(long, default_value = "plan")]
        target: String,
        /// environment: applies the `[env.<name>]` overrides
        #[arg(long, default_value = "local")]
        env: String,
        /// the published schema of the neutral plan, instead of a plan. It is
        /// what every `axon-infra-*` receives on stdin, and until now had to be
        /// deduced from an example
        #[arg(long)]
        schema: bool,
    },
    /// manifests -> mermaid: event topology
    Graph { sources: Vec<String> },
    /// manifests -> mermaid: class diagram
    Classes { sources: Vec<String> },
    /// the domain's state machines -> mermaid: stateDiagram
    States { sources: Vec<String> },
    /// migrations -> mermaid: entity-relationship
    Er { sources: Vec<String> },
    /// an event's causal flow -> mermaid: sequence
    Seq {
        event: String,
        sources: Vec<String>,
        /// Only the event chain, comparable with `axon trace --seq`.
        #[arg(long)]
        events: bool,
    },
    /// registry of services and methods (directory, file or URL)
    Discover { sources: Vec<String> },
    /// drift between manifests, migrations and infrastructure
    Verify { sources: Vec<String> },
    /// language server over stdio: `verify` as diagnostics, inside the editor
    Lsp,
    /// AsyncAPI or OpenAPI (JSON or YAML) -> an axon manifest
    Import {
        /// source format
        #[arg(value_parser = ["asyncapi", "openapi"])]
        format: String,
        /// file, or `-` for stdin
        file: String,
        /// service name, unless it has to be inferred from info.title
        #[arg(long)]
        service: Option<String>,
    },
    /// warehouse schemas and funnel views, derived from the events
    Analytics {
        sources: Vec<String>,
        #[arg(long, default_value = "bigquery",
              value_parser = ["bigquery", "snowflake", "clickhouse", "plan"])]
        target: String,
        /// emits the local target's loader instead of the schema: it carries the
        /// envelope log into the warehouse. ClickHouse only for now.
        #[arg(long)]
        load: Option<String>,
        /// the warehouse database, for the loader and for the introspection query
        #[arg(long, default_value = "axon")]
        dataset: String,
        /// emits what a Metabase needs to read what the manifest declares:
        /// the connection and one question per declared metric and funnel
        #[arg(long)]
        metabase: bool,
        /// emits the query that dumps the warehouse's REAL schema. Its output
        /// comes back through `--check`.
        #[arg(long)]
        introspect: bool,
        /// compares the declared against the warehouse dump. A new field the
        /// table does not have loads as nothing, and nobody sees an error.
        /// With `--metabase`, compares the questions somebody exported from
        /// the dashboard instead.
        #[arg(long)]
        check: Option<PathBuf>,
        /// emits the Vector config: the ingest path for a cluster, where there
        /// is no managed warehouse to subscribe to.
        #[arg(long)]
        vector: bool,
    },
    /// the API's maintenance cycle: what each version is, and what changed
    Versions { sources: Vec<String> },
    /// a pact from a consumer that does not use axon, crossed against what
    /// the provider declares
    Pact {
        sources: Vec<String>,
        /// the pact file (Pact v2, v3 or v4). axon does not need a broker to
        /// read one: what it wants is the list of fields the consumer needs
        #[arg(long)]
        check: PathBuf,
    },
    /// who calls what, read from the edge's access log
    Traffic {
        sources: Vec<String>,
        /// the edge's access log, NDJSON. axon does not connect to the edge:
        /// the log comes in here and the compiler crosses it against what the
        /// manifests declare
        #[arg(long)]
        check: PathBuf,
    },
    /// the system as it is, drawn: topology, verdict, versions and changes
    Tui {
        sources: Vec<String>,
        /// renders N frames to stdout and exits instead of taking over the
        /// terminal; that is what makes the picture checkable in CI
        #[arg(long)]
        frames: Option<u64>,
    },
    /// rules over a metric: the SQL that evaluates them, and what they propose
    Rules {
        sources: Vec<String>,
        /// the warehouse's answer (TSV) to compare the declared against what
        /// happened; without it the query is emitted
        #[arg(long)]
        check: Option<PathBuf>,
        /// MOVES the levers, on the flagd configuration at this path. Two locks
        /// and not one: the rule has to say `mode = "apply"` and you have to
        /// say this. Every change is appended to an audit trail next to it
        #[arg(long)]
        apply: Option<PathBuf>,
    },
    /// reconciles the declared CAP side with the patterns in use
    Cap {
        sources: Vec<String>,
        /// limits the report to these services; the analysis still looks at all of them
        #[arg(long = "service", short = 's')]
        services: Vec<String>,
    },
    /// flagd config derived from the declared `[flags.*]`
    Flags { sources: Vec<String> },
    /// load test derived from the manifest, and its verdict
    Load {
        manifest: PathBuf,
        /// k6 summary (`--summary-export`) to compare the measured against the
        /// declared; without it the script is emitted
        #[arg(long)]
        check: Option<PathBuf>,
    },
    /// snapshot of the published contracts, to detect incompatible changes
    Baseline { sources: Vec<String> },
    /// the warnings this repo lives with for now: with the file present, a new
    /// one fails the build. It is how an existing codebase can adopt `verify`
    /// without fixing two hundred things first
    Accept { sources: Vec<String> },
    /// pooler or sharder config, derived from the manifest
    Pooler {
        sources: Vec<String>,
        /// `local` names the containers `axon infra --target local` brings up;
        /// the rest leave the hosts as environment variables. `k8s` is what
        /// `axon infra --target k8s` puts in the ConfigMap, and the generated
        /// file's own header names this command: it has to be runnable.
        #[arg(long, default_value = "plan", value_parser = ["plan", "local", "k8s"])]
        target: String,
        /// emits the `users.toml` instead of the `pgdog.toml`: pgdog reads them as
        /// dos archivos separados
        #[arg(long)]
        users: bool,
        /// which service. Each one carries its own pgdog.toml, so it can only
        /// be omitted when a single one declares a pooler.
        #[arg(long = "service", short = 's')]
        service: Option<String>,
    },
    /// a project that verifies clean and comes up: manifest, migration,
    /// Dockerfile and policy, in the layout the rest of the CLI expects
    Init {
        /// the service's name: lowercase and dashes
        service: String,
        /// where to write it. The current directory by default
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// the verifier for what `[auth]` declares. Emitted, not linked: a file you
    /// can read beats a dependency that hides which claim it trusted
    Auth {
        manifest: String,
        #[arg(long, default_value = "ts", value_parser = ["ts"])]
        lang: String,
    },
    /// what a `[crud.*]` stands for, as TOML you can paste and edit
    Crud {
        manifest: String,
        /// prints the generated methods instead of nothing
        #[arg(long)]
        expand: bool,
    },
    /// the declared lists: table, seed and type from one place
    Catalog {
        sources: Vec<String>,
        /// which service. Each one's catalogs go in its own migration
        /// directory, so it can only be omitted when a single one declares any.
        #[arg(long = "service", short = 's')]
        service: Option<String>,
    },
    /// data access policies: per-row RLS and masked views
    Rls {
        sources: Vec<String>,
        /// `sql` protects the live query; `pg_anon` generates the dictionary for
        /// making a masked copy.
        #[arg(long, default_value = "sql", value_parser = ["sql", "pg_anon"])]
        target: String,
    },
    /// manifests -> OpenAPI 3.1 (one catalogue for the whole platform)
    Openapi {
        sources: Vec<String>,
        /// the document as of a dated version (`[api] versioning = "header"`):
        /// the shapes are the ones that version promised, not today's
        #[arg(long = "api-version")]
        api_version: Option<String>,
    },
    /// manifest -> test scaffolding (unit, integration, e2e)
    Test {
        manifest: PathBuf,
        sources: Vec<String>,
        #[arg(long, default_value = "ts")]
        lang: String,
        /// Path of the module `axon build` generated.
        #[arg(long, default_value = "./contracts.ts")]
        contracts: String,
    },
    /// NDJSON envelope log -> the real causal chain (for local debugging)
    Trace {
        /// file, or `-` for stdin
        #[arg(default_value = "-")]
        log: String,
        /// one business flow only
        #[arg(long)]
        correlation: Option<String>,
        /// mermaid instead of a tree, to diff against `axon seq`
        #[arg(long)]
        seq: bool,
        /// the manifests, to cross the REAL edges between services against the
        /// declared ones. It is the half `axon traffic` cannot see: a call
        /// between services does not pass through the edge
        #[arg(long = "manifests")]
        manifests: Vec<String>,
    },
}

/// The manifests and the directory the repo's own files live in. `verify` and
/// `accept` have to look at the same place, or the list of accepted warnings
/// would be written against a different report than the one that reads it.
fn discover_with_root(sources: &[String]) -> Result<(Vec<manifest::Manifest>, PathBuf), String> {
    let ms = manifest::discover(sources)?;
    let first = PathBuf::from(sources.first().map(|s| s.as_str()).unwrap_or("."));
    let root = if first.is_dir() {
        first
    } else {
        first
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf()
    };
    Ok((ms, root))
}

/// Everything `verify` knows: the rules, the published contracts and whatever
/// the check plugins say.
fn full_report(ms: &[manifest::Manifest], root: &std::path::Path) -> verify::Report {
    let mut r = verify::verify(ms, &verify::load_policy(root));
    if let Some(b) = baseline::cargar(root) {
        let (errors, warnings) = baseline::comparar(ms, &b);
        r.errors.extend(errors);
        r.warnings.extend(warnings);
    } else {
        r.warnings.push(format!(
            "no {}: `verify` cannot detect a breaking change in an already published \
             version. Generate it with `axon baseline`",
            baseline::ARCHIVO
        ));
    }
    let payload = serde_json::to_string(ms).unwrap_or_default();
    for bin in plugin::checks() {
        match plugin::run(&bin, &payload) {
            Ok(out) => match serde_json::from_str::<Vec<plugin::Finding>>(&out) {
                Ok(fs) => {
                    for f in fs {
                        let line = format!("[{bin}] {}", f.message);
                        if f.level == "error" {
                            r.errors.push(line)
                        } else {
                            r.warnings.push(line)
                        }
                    }
                }
                Err(e) => r.warnings.push(format!("[{bin}] invalid output: {e}")),
            },
            Err(e) => r.warnings.push(format!("[{bin}] did not run: {e}")),
        }
    }
    r
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("axon: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Build {
            manifest,
            sources,
            lang,
        } => {
            let m = manifest::load_any(&manifest)?;
            let all = if sources.is_empty() {
                vec![]
            } else {
                manifest::discover(&sources)?
            };
            if lang == "ts" {
                println!("{}", emit::build_ts(&m, &all)?);
            } else if lang == "go" {
                print!("{}", gen_go::build(&m, &all)?);
            } else {
                // a native target; the rest through a plugin
                let bin = format!("axon-gen-{lang}");
                if !plugin::exists(&bin) {
                    return Err(format!(
                        "`{bin}` is not on the PATH. A generator is any executable that \
                         reads {{manifest, peers}} on stdin and writes code on stdout."
                    ));
                }
                // The plugin receives the same as the native generator: its own
                // manifest and the others', because the schema of a consumed
                // event is owned by its emitter.
                let entry = serde_json::json!({ "manifest": m, "peers": all });
                print!("{}", plugin::run(&bin, &entry.to_string())?);
            }
        }
        Cmd::Ci {
            manifest,
            target,
            forge,
        } => {
            let m = manifest::load(&manifest)?;
            let dir = manifest.parent().unwrap_or(std::path::Path::new("."));
            let pol = verify::load_policy(dir);
            println!("{}", emit::build_ci(&m, &pol.ci, &target, &forge)?);
        }
        Cmd::Infra {
            sources,
            target,
            env,
            schema,
        } => {
            if schema {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&infra::plan_schema())
                        .map_err(|e| e.to_string())?
                );
                return Ok(ExitCode::SUCCESS);
            }
            let ms: Vec<_> = manifest::discover(&sources)?
                .iter()
                .map(|m| manifest::for_env(m, &env))
                .collect();
            let p = infra::plan(&ms);
            let bin = format!("axon-infra-{target}");
            if !infra::NATIVE.contains(&target.as_str()) && plugin::exists(&bin) {
                print!(
                    "{}",
                    plugin::run(&bin, &serde_json::to_string(&p).unwrap())?
                );
            } else {
                println!("{}", infra::render(&p, &target)?);
            }
        }
        Cmd::Graph { sources } => println!("{}", emit::build_graph(&manifest::discover(&sources)?)),
        Cmd::Classes { sources } => {
            println!("{}", emit::build_classes(&manifest::discover(&sources)?))
        }
        Cmd::States { sources } => {
            println!("{}", emit::build_states(&manifest::discover(&sources)?))
        }
        Cmd::Er { sources } => println!("{}", emit::build_er(&manifest::discover(&sources)?)),
        Cmd::Seq {
            event,
            sources,
            events,
        } => {
            println!(
                "{}",
                emit::build_seq(&manifest::discover(&sources)?, &event, events)?
            )
        }
        Cmd::Discover { sources } => {
            let ms = manifest::discover(&sources)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&registry(&ms)).map_err(|e| e.to_string())?
            );
        }
        Cmd::Verify { sources } => {
            let (ms, root) = discover_with_root(&sources)?;
            let mut r = full_report(&ms, &root);

            // The accepted warnings, if the repo drew that line. Its presence
            // IS the opt-in: with the file there, a warning that is not on the
            // list stops being a suggestion and fails the build.
            let list = accepted::cargar(&root);
            let (nuevas, vigentes, stale) = match &list {
                Some(a) => accepted::partir(&r.warnings, a),
                None => (r.warnings.iter().collect(), 0, vec![]),
            };
            let bloquea = list.is_some() && !nuevas.is_empty();

            // Errors first: they are what has to be fixed, and in a long list
            // what matters cannot end up at the bottom.
            for e in &r.errors {
                eprintln!("{} {}", color::red("error"), highlight(e));
            }
            for w in &nuevas {
                if bloquea {
                    eprintln!("{} {}", color::red("new"), highlight(w));
                } else {
                    println!("{}  {}", color::yellow("warn"), highlight(w));
                }
            }
            // The list can only shrink without anybody noticing. Saying which
            // entries no longer happen is what keeps it from becoming the place
            // warnings go to be forgotten.
            if !stale.is_empty() {
                println!(
                    "{}  {}; run `axon accept` to shrink the list",
                    color::green("gone"),
                    if stale.len() == 1 {
                        "1 accepted warning no longer happens".to_string()
                    } else {
                        format!("{} accepted warnings no longer happen", stale.len())
                    }
                );
            }
            if vigentes > 0 {
                println!(
                    "{}",
                    color::grey(&format!(
                        "      {vigentes} warnings accepted in {}",
                        accepted::ARCHIVO
                    ))
                );
            }
            // Two whole sentences and not one with a hole in it: the test that
            // checks the book quotes what the tool prints compares words, and a
            // word that only exists at runtime cannot be found in `src/`.
            let counted = if list.is_some() {
                nuevas.len()
            } else {
                r.warnings.len()
            };
            // Positional holes and not named ones: the test that checks the book
            // quotes what the tool prints compares the words of the literal, and
            // a `{name}` in the middle splits the sentence in two.
            let summary = if vigentes > 0 {
                format!(
                    "{} services, {} errors, {} warnings ({} accepted)",
                    ms.len(),
                    r.errors.len(),
                    counted,
                    vigentes
                )
            } else {
                format!(
                    "{} services, {} errors, {} warnings",
                    ms.len(),
                    r.errors.len(),
                    counted
                )
            };
            let clean = r.errors.is_empty() && nuevas.is_empty();
            if clean {
                println!("{} {}", color::green("ok"), color::grey(&summary));
            } else if r.errors.is_empty() && !bloquea {
                println!("{}  {}", color::yellow("near"), color::grey(&summary));
            } else {
                println!("{} {}", color::red("fail"), color::grey(&summary));
            }
            if !r.errors.is_empty() || bloquea {
                return Ok(ExitCode::FAILURE);
            }
            r.warnings.clear();
        }
        Cmd::Lsp => lsp::serve()?,
        Cmd::Import {
            format,
            file,
            service,
        } => {
            let text = if file == "-" {
                std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?
            } else {
                std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"))?
            };
            let manifest = match format.as_str() {
                "openapi" => import::openapi(&text, service.as_deref())?,
                _ => import::asyncapi(&text, service.as_deref())?,
            };
            print!("{manifest}");
        }
        Cmd::Analytics {
            sources,
            target,
            metabase,
            load,
            dataset,
            introspect,
            check,
            vector,
        } => {
            let ms = manifest::discover(&sources)?;
            if let Some(log) = load {
                print!("{}", bi::loader(&ms, &dataset, &log));
                return Ok(ExitCode::SUCCESS);
            }
            if vector {
                print!("{}", bi::vector(&ms, &dataset));
                return Ok(ExitCode::SUCCESS);
            }
            if metabase {
                // With `--check`, the other direction: what came back from the
                // Metabase, crossed against what axon generates. A question
                // written by hand against an axon table was invisible until now.
                let Some(route) = check else {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&bi::metabase(&ms, &dataset))
                            .map_err(|e| e.to_string())?
                    );
                    return Ok(ExitCode::SUCCESS);
                };
                let exported = std::fs::read_to_string(&route)
                    .map_err(|e| format!("{}: {e}", route.display()))?;
                let (errors, warnings, report) = bi::metabase_review(&ms, &dataset, &exported)?;
                println!("{report}");
                for a in &warnings {
                    println!("{}  {}", color::yellow("warn"), highlight(a));
                }
                for e in &errors {
                    eprintln!("{} {}", color::red("error"), highlight(e));
                }
                println!(
                    "{} {}",
                    if errors.is_empty() {
                        color::green("ok")
                    } else {
                        color::red("fail")
                    },
                    color::grey(&format!(
                        "{} question(s) against axon tables, {} errors, {} warnings",
                        report.lines().count().saturating_sub(1),
                        errors.len(),
                        warnings.len()
                    ))
                );
                return Ok(if errors.is_empty() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                });
            }
            if introspect || check.is_some() {
                let d =
                    bi::dialect(&target).ok_or_else(|| format!("unknown warehouse `{target}`"))?;
                if introspect {
                    print!("{}", bi::introspect(&d, &dataset));
                    return Ok(ExitCode::SUCCESS);
                }
                let route = check.unwrap();
                let real = std::fs::read_to_string(&route)
                    .map_err(|e| format!("{}: {e}", route.display()))?;
                let (errors, warnings) = bi::review(&ms, &d, &real);
                for a in &warnings {
                    eprintln!("{}", color::yellow(&format!("warn: {a}")));
                }
                for e in &errors {
                    eprintln!("{}", color::red(&format!("error: {e}")));
                }
                println!(
                    "axon: the warehouse has {} {} against the manifest",
                    errors.len(),
                    if errors.len() == 1 {
                        "difference"
                    } else {
                        "differences"
                    }
                );
                return Ok(if errors.is_empty() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                });
            }
            match target.as_str() {
                "plan" => println!(
                    "{}",
                    serde_json::to_string_pretty(&bi::build_plan(&ms)).map_err(|e| e.to_string())?
                ),
                other => {
                    let d =
                        bi::dialect(other).ok_or_else(|| format!("unknown warehouse `{other}`"))?;
                    print!("{}", bi::build(&ms, &d));
                }
            }
        }
        Cmd::Pact { sources, check } => {
            let ms = manifest::discover(&sources)?;
            let text =
                std::fs::read_to_string(&check).map_err(|e| format!("{}: {e}", check.display()))?;
            let p = pact::parse(&text)?;
            let (errors, warnings, report) = pact::review(&ms, &p);
            if !report.is_empty() {
                println!("{report}");
            }
            for w in &warnings {
                println!("{}  {}", color::yellow("warn"), highlight(w));
            }
            for e in &errors {
                eprintln!("{} {}", color::red("error"), highlight(e));
            }
            println!(
                "{} {}",
                if errors.is_empty() {
                    color::green("ok")
                } else {
                    color::red("fail")
                },
                color::grey(&format!(
                    "{} interactions, {} messages, {} errors, {} warnings",
                    p.expectations.len(),
                    p.messages.len(),
                    errors.len(),
                    warnings.len()
                ))
            );
            if !errors.is_empty() {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::Traffic { sources, check } => {
            let ms = manifest::discover(&sources)?;
            let text =
                std::fs::read_to_string(&check).map_err(|e| format!("{}: {e}", check.display()))?;
            let hits = traffic::parse(&text);
            let (report, fails) = traffic::report(&ms, &hits);
            print!("{report}");
            if fails {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::Tui { sources, frames } => {
            let ms = manifest::discover(&sources)?;
            let first = sources
                .first()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let root = if first.is_dir() {
                first
            } else {
                first
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .to_path_buf()
            };
            match frames {
                Some(n) => print!("{}", tui::frames(&ms, &root, n)?),
                None => tui::run(&ms, &root)?,
            }
        }
        Cmd::Rules {
            sources,
            check,
            apply,
        } => {
            let ms = manifest::discover(&sources)?;
            match check {
                None => {
                    let warehouse = ms
                        .iter()
                        .find(|m| !m.external)
                        .map(|m| m.analytics.warehouse.clone())
                        .unwrap_or_else(|| "bigquery".into());
                    let d = bi::dialect(&warehouse)
                        .ok_or_else(|| format!("unknown warehouse `{warehouse}`"))?;
                    print!("{}", bi::rules_sql(&ms, &d));
                }
                Some(file) => {
                    let text = std::fs::read_to_string(&file)
                        .map_err(|e| format!("{}: {e}", file.display()))?;
                    let windows = bi::parse_windows(&text);
                    let proposals = bi::decide(&ms, &windows);
                    if proposals.is_empty() {
                        println!("axon: no rules declared");
                        return Ok(ExitCode::SUCCESS);
                    }
                    let mut firing = 0;
                    for p in &proposals {
                        if p.fires {
                            firing += 1;
                            println!(
                                "{}  {}.{}",
                                color::yellow("proposes"),
                                p.service,
                                color::bold(&p.rule)
                            );
                            println!("  {}", p.action);
                            println!("  {}", color::grey(&p.why));
                        } else {
                            println!(
                                "{}     {}.{}  {}",
                                color::grey("quiet"),
                                p.service,
                                p.rule,
                                color::grey(&p.why)
                            );
                        }
                    }
                    // Nothing was applied unless BOTH locks are open: the
                    // rule says it may be, and whoever runs this says now.
                    let Some(flags_path) = apply else {
                        println!(
                            "axon: {firing} of {} rules propose a change; none was applied",
                            proposals.len()
                        );
                        return Ok(ExitCode::SUCCESS);
                    };
                    let text = std::fs::read_to_string(&flags_path)
                        .map_err(|e| format!("{}: {e}", flags_path.display()))?;
                    let mut flags: serde_json::Value =
                        serde_json::from_str(&text).map_err(|e| {
                            format!("{}: not a flagd configuration: {e}", flags_path.display())
                        })?;
                    // The clock comes from the system and the format is the one
                    // an envelope uses, so an audit line and a trace can be put
                    // side by side without translating anything.
                    let (y, mo, d) = manifest::today();
                    let when = format!("{y:04}-{mo:02}-{d:02}");
                    let (moved, audit) = bi::apply(&proposals, &mut flags, &when);
                    for line in &moved {
                        println!("{} {}", color::yellow("applied"), highlight(line));
                    }
                    if !audit.is_empty() {
                        std::fs::write(
                            &flags_path,
                            format!(
                                "{}\n",
                                serde_json::to_string_pretty(&flags).map_err(|e| e.to_string())?
                            ),
                        )
                        .map_err(|e| format!("{}: {e}", flags_path.display()))?;
                        // The trail lives next to what it describes, and it is
                        // appended and never rewritten.
                        let trail = flags_path.with_extension("audit.ndjson");
                        let mut f = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&trail)
                            .map_err(|e| format!("{}: {e}", trail.display()))?;
                        use std::io::Write;
                        for line in &audit {
                            writeln!(f, "{line}").map_err(|e| e.to_string())?;
                        }
                        println!(
                            "{}",
                            color::grey(&format!(
                                "      {} change(s) written to {} and to {}",
                                audit.len(),
                                flags_path.display(),
                                trail.display()
                            ))
                        );
                    }
                    println!(
                        "axon: {firing} of {} rules propose a change; {} applied",
                        proposals.len(),
                        audit.len()
                    );
                }
            }
        }
        Cmd::Versions { sources } => {
            print!("{}", versions::report(&manifest::discover(&sources)?))
        }
        Cmd::Cap { sources, services } => {
            println!(
                "{}",
                cap::informe(&manifest::discover(&sources)?, &services)
            )
        }
        Cmd::Flags { sources } => {
            print!("{}", emit::build_flagd(&manifest::discover(&sources)?))
        }
        Cmd::Load { manifest, check } => {
            let m = manifest::load(&manifest)?;
            match check {
                None => println!("{}", carga::build_k6(&m)?),
                Some(f) => {
                    let json =
                        std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                    let (errors, warnings) = carga::review(&m, &json)?;
                    for a in &warnings {
                        println!("info: {a}");
                    }
                    for e in &errors {
                        eprintln!("error: {e}");
                    }
                    println!("axon: {} thresholds breached", errors.len());
                    if !errors.is_empty() {
                        return Ok(ExitCode::FAILURE);
                    }
                }
            }
        }
        Cmd::Accept { sources } => {
            let (ms, root) = discover_with_root(&sources)?;
            let r = full_report(&ms, &root);
            let a = accepted::tomar(&ms, &r.warnings);
            println!(
                "{}",
                serde_json::to_string_pretty(&a).map_err(|e| e.to_string())?
            );
        }
        Cmd::Baseline { sources } => {
            let b = baseline::tomar(&manifest::discover(&sources)?);
            println!(
                "{}",
                serde_json::to_string_pretty(&b).map_err(|e| e.to_string())?
            );
        }
        Cmd::Pooler {
            sources,
            target,
            users,
            service,
        } => {
            let ms = manifest::discover(&sources)?;
            let solo = service.as_deref();
            print!(
                "{}",
                match users {
                    true => pooler::users(&ms, &target, solo)?,
                    false => pooler::build(&ms, &target, solo)?,
                }
            )
        }
        Cmd::Init { service, path } => {
            print!("{}", init::run(&path, &service)?)
        }
        Cmd::Auth { manifest, lang } => {
            let _ = lang;
            let m = manifest::load_any(&manifest)?;
            print!("{}", emit::verifier_ts(&m)?)
        }
        Cmd::Crud { manifest, expand } => {
            let m = manifest::load_any(&manifest)?;
            if !expand {
                return Err("nothing to do without `--expand`".into());
            }
            print!("{}", catalog::expanded_toml(&m)?)
        }
        Cmd::Catalog { sources, service } => {
            let ms = manifest::discover(&sources)?;
            print!("{}", catalog::build(&ms, service.as_deref())?)
        }
        Cmd::Rls { sources, target } => {
            let ms = manifest::discover(&sources)?;
            print!(
                "{}",
                match target.as_str() {
                    "pg_anon" => dbsec::build_pg_anon(&ms),
                    _ => dbsec::build(&ms),
                }
            )
        }
        Cmd::Openapi {
            sources,
            api_version,
        } => {
            let ms = manifest::discover(&sources)?;
            if let Some(v) = &api_version {
                let api = ms.iter().find(|m| !m.external).map(|m| &m.api);
                if api.is_none_or(|a| a.find(v).is_none()) {
                    return Err(format!(
                        "`{v}` is not a declared version of the API. Declared: {}",
                        api.map(|a| a.dates().join(", "))
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| "none".into())
                    ));
                }
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&api::openapi_at(&ms, api_version.as_deref()))
                    .map_err(|e| e.to_string())?
            )
        }
        Cmd::Test {
            manifest,
            sources,
            lang,
            contracts,
        } => {
            if lang != "ts" {
                return Err(format!("lang `{lang}` has no native generator"));
            }
            let target = manifest::load(&manifest)?;
            let all = if sources.is_empty() {
                vec![target.clone()]
            } else {
                manifest::discover(&sources)?
            };
            println!("{}", api::build_tests(&all, &target, &contracts)?);
        }
        Cmd::Trace {
            log,
            correlation,
            seq,
            manifests,
        } => {
            let text = if log == "-" {
                std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?
            } else {
                std::fs::read_to_string(&log).map_err(|e| format!("{log}: {e}"))?
            };
            // The envelope log, OTLP or Jaeger: a span is an envelope with
            // other names, so from here on it is the same code.
            let (evs, from) = trace::parse_any(&text);
            let c = correlation.as_deref();
            println!(
                "{}",
                if seq {
                    trace::sequence(&evs, c)
                } else {
                    trace::tree(&evs, c)
                }
            );
            if !manifests.is_empty() {
                let ms = manifest::discover(&manifests)?;
                println!("{}", color::grey(&format!("read from the {from}")));
                let (report, fails) = trace::edges(&evs, &ms);
                print!("{report}");
                if fails {
                    return Ok(ExitCode::FAILURE);
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Highlights whatever sits between backticks. The messages are already
/// written with `` `this` `` to name fields and values; this takes advantage of
/// that instead of asking for a new format.
fn highlight(msg: &str) -> String {
    // With no colour the message comes out as-is: stripping the backticks would
    // change the content, and a highlight must not change what the text says. I
    // found that out by breaking nine tests that check the messages.
    if !color::enabled() {
        return msg.to_string();
    }
    let mut out = String::with_capacity(msg.len());
    let mut dentro = false;
    for parte in msg.split('`') {
        if dentro {
            out.push_str(&color::bold(parte));
        } else {
            out.push_str(parte);
        }
        dentro = !dentro;
    }
    out
}

fn registry(ms: &[manifest::Manifest]) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for m in ms {
        out.insert(
            m.service.clone(),
            serde_json::json!({
                "version": m.version,
                "external": m.external,
                "source": m.origin.to_string_lossy(),
                "methods": m.methods,
                "emits": m.emits.keys().collect::<Vec<_>>(),
                "consumes": m.consumes.keys().collect::<Vec<_>>(),
            }),
        );
    }
    serde_json::Value::Object(out)
}
