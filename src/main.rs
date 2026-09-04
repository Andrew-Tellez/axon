//! axon — el manifiesto es la fuente de verdad; el resto son proyecciones.
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
mod trace;
mod verify;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "axon",
    version,
    about = "Compilador manifiesto-primero para microservicios event-driven"
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
        /// Los demas manifiestos: de ahi sale el tipo de lo que este servicio consume.
        sources: Vec<String>,
        #[arg(long, default_value = "ts")]
        lang: String,
    },
    /// manifiesto -> pipeline de CI/CD
    Ci {
        manifest: PathBuf,
        /// plataforma de despliegue; sin esto solo genera los gates
        #[arg(long, default_value = "none")]
        target: String,
    },
    /// manifiestos -> IaC. `--target plan` da el plan neutral en JSON.
    Infra {
        sources: Vec<String>,
        #[arg(long, default_value = "plan")]
        target: String,
        /// entorno: aplica los overrides de `[env.<nombre>]`
        #[arg(long, default_value = "local")]
        env: String,
    },
    /// manifiestos -> mermaid: topologia de eventos
    Graph { sources: Vec<String> },
    /// manifiestos -> mermaid: diagrama de clases
    Classes { sources: Vec<String> },
    /// maquinas de estado del dominio -> mermaid: stateDiagram
    States { sources: Vec<String> },
    /// migraciones -> mermaid: entidad-relacion
    Er { sources: Vec<String> },
    /// flujo causal de un evento -> mermaid: secuencia
    Seq {
        event: String,
        sources: Vec<String>,
        /// Solo la cadena de eventos, comparable con `axon trace --seq`.
        #[arg(long)]
        events: bool,
    },
    /// registro de servicios y metodos (directorio, archivo o URL)
    Discover { sources: Vec<String> },
    /// drift entre manifiestos, migraciones e infraestructura
    Verify { sources: Vec<String> },
    /// AsyncAPI (2.x o 3.x, JSON o YAML) -> manifiesto axon
    Import {
        /// formato de origen
        #[arg(value_parser = ["asyncapi"])]
        formato: String,
        /// archivo, o `-` para stdin
        file: String,
        /// nombre del servicio, si no hay que deducirlo de info.title
        #[arg(long)]
        service: Option<String>,
    },
    /// esquemas de bodega y vistas de embudo, derivados de los eventos
    Analytics {
        sources: Vec<String>,
        #[arg(long, default_value = "bigquery",
              value_parser = ["bigquery", "snowflake", "clickhouse", "plan"])]
        target: String,
    },
    /// reconcilia el lado CAP declarado con los patrones en uso
    Cap {
        sources: Vec<String>,
        /// limita el informe a estos servicios; el analisis igual mira a todos
        #[arg(long = "service", short = 's')]
        services: Vec<String>,
    },
    /// configuracion de flagd derivada de los `[flags.*]` declarados
    Flags { sources: Vec<String> },
    /// prueba de carga derivada del manifiesto, y su veredicto
    Load {
        manifest: PathBuf,
        /// resumen de k6 (`--summary-export`) para comparar lo medido con lo
        /// declarado; sin esto emite el script
        #[arg(long)]
        check: Option<PathBuf>,
    },
    /// snapshot de los contratos publicados, para detectar cambios incompatibles
    Baseline { sources: Vec<String> },
    /// politicas de acceso a datos: RLS por fila y vistas enmascaradas
    Rls {
        sources: Vec<String>,
        /// `sql` protege la consulta viva; `pg_anon` genera el diccionario para
        /// hacer una copia enmascarada.
        #[arg(long, default_value = "sql", value_parser = ["sql", "pg_anon"])]
        target: String,
    },
    /// manifiestos -> OpenAPI 3.1 (un catalogo para toda la plataforma)
    Openapi { sources: Vec<String> },
    /// manifiesto -> andamiaje de pruebas (unitarias, integracion, e2e)
    Test {
        manifest: PathBuf,
        sources: Vec<String>,
        #[arg(long, default_value = "ts")]
        lang: String,
        /// Ruta del modulo que genero `axon build`.
        #[arg(long, default_value = "./contracts.ts")]
        contracts: String,
    },
    /// log NDJSON de envelopes -> cadena causal real (para debug local)
    Trace {
        /// archivo, o `-` para stdin
        #[arg(default_value = "-")]
        log: String,
        /// solo un flujo de negocio
        #[arg(long)]
        correlation: Option<String>,
        /// mermaid en vez de arbol, para diffear contra `axon seq`
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
                // un target nativo; el resto por plugin
                let bin = format!("axon-gen-{lang}");
                if !plugin::exists(&bin) {
                    return Err(format!(
                        "`{bin}` no esta en el PATH. Un generador es cualquier ejecutable \
                         que lea {{manifest, peers}} por stdin y escriba codigo por stdout."
                    ));
                }
                // El plugin recibe lo mismo que el generador nativo: su
                // manifiesto y el de los demas, porque el esquema de un evento
                // consumido lo posee su emisor.
                let entrada = serde_json::json!({ "manifest": m, "peers": all });
                print!("{}", plugin::run(&bin, &entrada.to_string())?);
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
            // los contratos publicados, si el repo los registra
            if let Some(b) = baseline::cargar(root) {
                let (errores, avisos) = baseline::comparar(&ms, &b);
                r.errors.extend(errores);
                r.warnings.extend(avisos);
            } else {
                r.warnings.push(format!(
                    "sin {}: `verify` no puede detectar un cambio incompatible en una \
                     version ya publicada. Generalo con `axon baseline`",
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
            // Los errores primero: son lo que hay que arreglar, y en una lista
            // larga lo importante no puede quedar debajo.
            for e in &r.errors {
                eprintln!("{} {}", color::rojo("error"), realzar(e));
            }
            for w in &r.warnings {
                println!("{}  {}", color::amarillo("aviso"), realzar(w));
            }
            let resumen = format!(
                "{} servicios, {} errores, {} avisos",
                ms.len(),
                r.errors.len(),
                r.warnings.len()
            );
            if r.errors.is_empty() && r.warnings.is_empty() {
                println!("{} {}", color::verde("ok"), color::gris(&resumen));
            } else if r.errors.is_empty() {
                println!("{}  {}", color::amarillo("casi"), color::gris(&resumen));
            } else {
                println!("{} {}", color::rojo("falla"), color::gris(&resumen));
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
        Cmd::Analytics { sources, target } => {
            let ms = manifest::discover(&sources)?;
            match target.as_str() {
                "plan" => println!(
                    "{}",
                    serde_json::to_string_pretty(&bi::build_plan(&ms)).map_err(|e| e.to_string())?
                ),
                otro => {
                    let d =
                        bi::dialecto(otro).ok_or_else(|| format!("bodega `{otro}` desconocida"))?;
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
                    let (errores, avisos) = carga::revisar(&m, &json)?;
                    for a in &avisos {
                        println!("info: {a}");
                    }
                    for e in &errores {
                        eprintln!("error: {e}");
                    }
                    println!("axon: {} umbrales incumplidos", errores.len());
                    if !errores.is_empty() {
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
                return Err(format!("lang `{lang}` sin generador nativo"));
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

/// Resalta lo que va entre acentos graves. Los mensajes ya se escriben con
/// `` `asi` `` para nombrar campos y valores; esto lo aprovecha en vez de
/// pedir un formato nuevo.
fn realzar(msg: &str) -> String {
    // Sin color, el mensaje sale tal cual: quitar los acentos graves cambiaria
    // el contenido, y un realce no debe cambiar lo que dice el texto. Lo
    // descubri rompiendo nueve pruebas que verifican los mensajes.
    if !color::activos() {
        return msg.to_string();
    }
    let mut out = String::with_capacity(msg.len());
    let mut dentro = false;
    for parte in msg.split('`') {
        if dentro {
            out.push_str(&color::fuerte(parte));
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
