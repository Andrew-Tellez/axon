//! axon — the manifest is the source of truth; the rest are projections.
mod api;
mod baseline;
mod bi;
mod cap;
mod carga;
mod color;
mod dbsec;
mod emit;
mod import;
mod infra;
mod manifest;
mod plugin;
mod pooler;
mod trace;
mod verify;

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
    /// manifiesto -> contratos y clase base
    Build {
        manifest: PathBuf,
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
    },
    /// manifests -> IaC. `--target plan` gives the neutral plan in JSON.
    Infra {
        sources: Vec<String>,
        #[arg(long, default_value = "plan")]
        target: String,
        /// environment: applies the `[env.<name>]` overrides
        #[arg(long, default_value = "local")]
        env: String,
    },
    /// manifests -> mermaid: event topology
    Graph { sources: Vec<String> },
    /// manifests -> mermaid: class diagram
    Classes { sources: Vec<String> },
    /// the domain's state machines -> mermaid: stateDiagram
    States { sources: Vec<String> },
    /// migraciones -> mermaid: entidad-relacion
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
    /// drift entre manifiestos, migraciones e infraestructura
    Verify { sources: Vec<String> },
    /// AsyncAPI (2.x o 3.x, JSON o YAML) -> manifiesto axon
    Import {
        /// source format
        #[arg(value_parser = ["asyncapi"])]
        formato: String,
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
        /// emits the query that dumps the warehouse's REAL schema. Its output
        /// comes back through `--check`.
        #[arg(long)]
        introspect: bool,
        /// compares the declared against the warehouse dump. A new field the
        /// table does not have loads as nothing, and nobody sees an error.
        #[arg(long)]
        check: Option<PathBuf>,
        /// emits the Vector config: the ingest path for a cluster, where there
        /// is no managed warehouse to subscribe to.
        #[arg(long)]
        vector: bool,
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
    /// pooler or sharder config, derived from the manifest
    Pooler {
        sources: Vec<String>,
        /// `local` names the containers `axon infra --target local` brings up;
        /// the rest leave the hosts as environment variables.
        #[arg(long, default_value = "plan", value_parser = ["plan", "local"])]
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
    /// data access policies: per-row RLS and masked views
    Rls {
        sources: Vec<String>,
        /// `sql` protects the live query; `pg_anon` generates the dictionary for
        /// making a masked copy.
        #[arg(long, default_value = "sql", value_parser = ["sql", "pg_anon"])]
        target: String,
    },
    /// manifests -> OpenAPI 3.1 (one catalogue for the whole platform)
    Openapi { sources: Vec<String> },
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
    },
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
            let m = manifest::load(&manifest)?;
            let all = if sources.is_empty() {
                vec![]
            } else {
                manifest::discover(&sources)?
            };
            if lang == "ts" {
                println!("{}", emit::build_ts(&m, &all)?);
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
        Cmd::Ci { manifest, target } => {
            let m = manifest::load(&manifest)?;
            let dir = manifest.parent().unwrap_or(std::path::Path::new("."));
            let pol = verify::load_policy(dir);
            println!("{}", emit::build_ci(&m, &pol.ci, &target));
        }
        Cmd::Infra {
            sources,
            target,
            env,
        } => {
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
            let ms = manifest::discover(&sources)?;
            let dir = std::path::Path::new(sources.first().map(|s| s.as_str()).unwrap_or("."));
            let root = if dir.is_dir() {
                dir
            } else {
                dir.parent().unwrap_or(std::path::Path::new("."))
            };
            let mut r = verify::verify(&ms, &verify::load_policy(root));
            // the published contracts, if the repo registers them
            if let Some(b) = baseline::cargar(root) {
                let (errors, warnings) = baseline::comparar(&ms, &b);
                r.errors.extend(errors);
                r.warnings.extend(warnings);
            } else {
                r.warnings.push(format!(
                    "no {}: `verify` cannot detect a breaking change in an already \
                     published version. Generate it with `axon baseline`",
                    baseline::ARCHIVO
                ));
            }
            let payload = serde_json::to_string(&ms).unwrap_or_default();
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
                        Err(e) => r.warnings.push(format!("[{bin}] salida invalida: {e}")),
                    },
                    Err(e) => r.warnings.push(format!("[{bin}] no corrio: {e}")),
                }
            }
            // Errors first: they are what has to be fixed, and in a long list
            // what matters cannot end up at the bottom.
            for e in &r.errors {
                eprintln!("{} {}", color::red("error"), realzar(e));
            }
            for w in &r.warnings {
                println!("{}  {}", color::yellow("warn"), realzar(w));
            }
            let resumen = format!(
                "{} services, {} errors, {} warnings",
                ms.len(),
                r.errors.len(),
                r.warnings.len()
            );
            if r.errors.is_empty() && r.warnings.is_empty() {
                println!("{} {}", color::green("ok"), color::grey(&resumen));
            } else if r.errors.is_empty() {
                println!("{}  {}", color::yellow("near"), color::grey(&resumen));
            } else {
                println!("{} {}", color::red("fail"), color::grey(&resumen));
            }
            if !r.errors.is_empty() {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::Import {
            formato: _,
            file,
            service,
        } => {
            let text = if file == "-" {
                std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?
            } else {
                std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"))?
            };
            print!("{}", import::asyncapi(&text, service.as_deref())?);
        }
        Cmd::Analytics {
            sources,
            target,
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
            if introspect || check.is_some() {
                let d = bi::dialect(&target)
                    .ok_or_else(|| format!("unknown warehouse `{target}`"))?;
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
        Cmd::Openapi { sources } => println!(
            "{}",
            serde_json::to_string_pretty(&api::openapi(&manifest::discover(&sources)?))
                .map_err(|e| e.to_string())?
        ),
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
        } => {
            let text = if log == "-" {
                std::io::read_to_string(std::io::stdin()).map_err(|e| e.to_string())?
            } else {
                std::fs::read_to_string(&log).map_err(|e| format!("{log}: {e}"))?
            };
            let evs = trace::parse(&text);
            let c = correlation.as_deref();
            println!(
                "{}",
                if seq {
                    trace::sequence(&evs, c)
                } else {
                    trace::tree(&evs, c)
                }
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Highlights whatever sits between backticks. The messages are already
/// written with `` `this` `` to name fields and values; this takes advantage of
/// that instead of asking for a new format.
fn realzar(msg: &str) -> String {
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
