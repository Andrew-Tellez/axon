//! `axon init`: a project that verifies clean and comes up.
//!
//! It exists because of what using the CLI on an empty directory found: the
//! first three failures were all layout. `migrations` resolves from the
//! manifest, so `manifests/` next to `sql/` reads nothing; the compose builds
//! `services/<svc>/Dockerfile`, which does not exist yet; and the `.env.local`
//! it referenced had no reason to exist either.
//!
//! Three messages can explain that. A scaffold makes it not happen.
use std::path::{Path, PathBuf};

/// One file, refusing to overwrite. `init` on a directory that already has
/// something is the case where clobbering is unforgivable.
fn write(root: &Path, rel: &str, body: &str, made: &mut Vec<String>) -> Result<(), String> {
    let path: PathBuf = root.join(rel);
    if path.exists() {
        return Err(format!(
            "{rel} already exists. `init` writes a project from scratch and never over one"
        ));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
    made.push(rel.to_string());
    Ok(())
}

pub fn run(root: &Path, service: &str) -> Result<String, String> {
    if service.is_empty() || !service.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
        return Err(format!(
            "`{service}` is not a service name: lowercase and dashes, because it becomes a \
             container name, a topic prefix, a database and a directory"
        ));
    }
    let mut made = Vec::new();
    // The manifest sits at the ROOT and not in `manifests/`, and that is the
    // whole point: `migrations` resolves from the manifest's own directory, so
    // the layout that needs no `../` is the one that gets written.
    write(
        root,
        &format!("{service}.toml"),
        &manifest(service),
        &mut made,
    )?;
    write(
        root,
        &format!("sql/{service}/001_{service}.sql"),
        &schema(service),
        &mut made,
    )?;
    write(
        root,
        &format!("services/{service}/Dockerfile"),
        &DOCKERFILE.replace("SERVICE", service),
        &mut made,
    )?;
    write(
        root,
        &format!("services/{service}/index.ts"),
        &index_ts(service),
        &mut made,
    )?;
    write(root, ".env.local", ENV_LOCAL, &mut made)?;
    write(root, "axon.policy.toml", POLICY, &mut made)?;
    write(root, ".gitignore", GITIGNORE, &mut made)?;
    Ok(format!(
        "{}\n\nWhat to do next, in this order:\n  \
         axon verify .                     # 0 errors: the layout is the one the CLI expects\n  \
         axon build {service}.toml . > services/{service}/contracts.ts\n  \
         axon infra . --target local > axon.local.yml\n  \
         docker compose -f axon.local.yml up -d --build --wait\n\n\
         The two warnings it starts with are the next two decisions, not defects:\n  \
         [auth]        where the subject, the tenant and the scopes are read from\n  \
         axon baseline records what is published, so a breaking change can be seen\n",
        made.iter()
            .map(|f| format!("  wrote {f}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn manifest(service: &str) -> String {
    format!(
        r#"service = "{service}"
owner = "your-team"
tier = "2"
version = "0.1.0"

# What this service promises when the network splits. It is the first thing
# `verify` reads, because half its rules are consequences of this pair.
[cap]
consistency = "eventual"
on_partition = "degrade"
max_staleness_ms = 5000

[infra]
state = "postgres"
# Relative to THIS file. The manifest lives at the root for that reason: with
# it under `manifests/`, this would have to be `../sql/{service}` and reading
# nothing is a silent failure.
migrations = "sql/{service}"
pool_size = 10
max_connections = 200

# The platform's scope catalogue. A scope with a typo is a 403 in production
# that nobody sees in a review, and this list is what catches it.
[api]
scopes = ["{service}:read", "{service}:write"]

# Exporting is ON by default and the default warehouse is BigQuery, so a
# service that does not export has to say so.
[analytics]
export = false

[methods.getThing]
http = "GET /v1/things/{{thingId}}"
auth = "required"
scopes = ["{service}:read"]
in = {{ thingId = "uuid" }}
out = {{ thingId = "uuid", name = "string" }}

# The id comes from the caller, so a retry cannot create a second row. axon
# refuses a mutation that is not idempotent for exactly that reason.
[methods.createThing]
http = "POST /v1/things"
auth = "required"
scopes = ["{service}:write"]
idempotent = true
in = {{ thingId = "uuid", name = "string" }}
out = {{ thingId = "uuid" }}
"#
    )
}

fn schema(service: &str) -> String {
    format!(
        "-- `001_<name>.sql`: three digits and an underscore, which is what the\n\
         -- generated pipeline hands Flyway. Flyway's own `V1__<name>.sql` works\n\
         -- too —axon reads the convention from the directory— and mixing the two\n\
         -- in one directory is refused, because then they have no defined order.\n\
         CREATE TABLE thing (\n  \
           id uuid PRIMARY KEY,\n  \
           name text NOT NULL\n\
         );\n\n\
         -- The outbox, if this service emits events in the same transaction as it\n\
         -- changes state. `axon verify` asks for it when `[patterns] outbox = true`.\n\
         -- {service} does not emit anything yet.\n"
    )
}

const DOCKERFILE: &str = r#"# The compose `axon infra --target local` writes builds THIS file. Without it,
# docker fails with a path error that names nothing, which is why `init` writes
# one.
FROM node:24-alpine
WORKDIR /app
# Node 24 runs TypeScript with no build step: the generated contracts are
# imported as .ts and the types are stripped at load.
COPY services ./services
CMD ["node", "--experimental-strip-types", "services/SERVICE/index.ts"]
"#;

fn index_ts(service: &str) -> String {
    format!(
        r#"// Your code. Everything that crosses a process boundary is generated; what is
// here is the part only you know.
//
//   axon build {service}.toml . > services/{service}/contracts.ts
//
// and then implement the abstract methods of the generated class.
import {{ createServer }} from "node:http";
import {{ httpRoutes }} from "./contracts.ts";

// The declared routes answer 501 and say WHERE to implement them. A stub that
// answered 200 with invented data would be the one thing this whole project
// exists to prevent: something that looks right and is not.
createServer((req, res) => {{
  if (req.url === "/healthz" || req.url === "/") {{
    res.writeHead(200, {{ "content-type": "application/json" }});
    return res.end(JSON.stringify({{ status: "ok" }}));
  }}
  const method = httpRoutes.find((r) => r.startsWith(`${{req.method}} `));
  res.writeHead(501, {{ "content-type": "application/problem+json" }});
  res.end(
    JSON.stringify({{
      type: "about:blank",
      title: "not_implemented",
      status: 501,
      detail: method
        ? `the manifest declares ${{method}}; implement it in services/{service}/index.ts`
        : `no declared route matches ${{req.method}} ${{req.url}}. What exists: ${{httpRoutes.join(", ")}}`,
    }}),
  );
}}).listen(8080, () => console.log("[{service}] listening on :8080"));
"#
    )
}

const ENV_LOCAL: &str = "# The secrets the manifest declares, and NOT their values in git.\n\
                         # `axon verify` refuses a value that looks like a real secret.\n";

const POLICY: &str = r#"# The team's governance, versioned. `axon verify` applies it.
require_owner = true
require_tier = true

# The repo's layout. `{service}` is substituted.
[ci]
manifests_dir  = "."
service_dir    = "services/{service}"
test_cmd       = "node --test services/{service}"
contracts_path = "services/{service}/contracts.ts"
"#;

const GITIGNORE: &str = "node_modules/\n.axon/\naxon.local.yml\n";
