//! The integration guide: what somebody OUTSIDE this service needs in order to
//! call it, written from the same manifest the service is compiled from.
//!
//! `openapi` already describes the routes and their shapes, and a generated
//! client can be built from it. What it does not carry is everything an
//! integrator asks before the first request: which issuer signs the token, what
//! the tenant claim is called, which events they can subscribe to instead of
//! polling, what happens when the other side is partitioned. All of that is
//! declared —in `[auth]`, `[emits]`, `[cap]`— and until now it only existed as
//! generated code nobody outside the repo reads.
//!
//! Markdown and not another JSON on purpose: the reader is a person onboarding
//! or an agent reading context, and both of them do better with one document
//! than with four commands whose outputs they have to join.
//!
//! Nothing here is authored. There is no `[docs]` block and there should not
//! be: prose that is not derived from a declaration is prose that goes stale
//! without anything failing, which is the exact problem the manifest exists to
//! solve.
use crate::manifest::*;

/// `field = "type"` as a Markdown table, or a line saying it carries nothing.
fn shape(f: &Fields) -> String {
    if f.is_empty() {
        return "_no fields_\n".into();
    }
    let mut s = String::from("| field | type |\n| --- | --- |\n");
    for (k, t) in f {
        s.push_str(&format!("| `{k}` | {t} |\n"));
    }
    s
}

fn auth_section(m: &Manifest) -> String {
    let a = &m.auth;
    if a.issuers.is_empty() && a.jwks_uri.is_none() && a.introspection_url.is_none() {
        return "## Authentication\n\nThis service declares no `[auth]`: it trusts whatever the \
                gateway put in front of it, and checks only the scopes below. Ask the platform \
                team which issuer that gateway accepts —it is not declared here, so this document \
                cannot tell you.\n\n"
            .into();
    }
    let mut s = String::from("## Authentication\n\nBearer token on every call.\n\n");
    s.push_str("| | |\n| --- | --- |\n");
    let mut row = |k: &str, v: String| s.push_str(&format!("| {k} | {v} |\n"));
    if !a.issuers.is_empty() {
        row(
            "accepted issuers",
            a.issuers
                .iter()
                .map(|i| format!("`{i}`"))
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if let Some(v) = &a.audience {
        row("audience", format!("`{v}`"));
    }
    if let Some(v) = &a.verify {
        row("verified by", format!("`{v}`"));
    }
    if let Some(v) = &a.jwks_uri {
        row("JWKS", format!("`{v}`"));
    }
    if let Some(v) = &a.introspection_url {
        row("introspection", format!("`{v}`"));
    }
    if !a.algorithms.is_empty() {
        row("algorithms", format!("`{}`", a.algorithms.join("`, `")));
    }
    if let Some(v) = a.max_token_age_s {
        row(
            "max token age",
            format!("{v}s — an older token is refused even if it has not expired"),
        );
    }
    if let Some(v) = a.clock_skew_s {
        row("tolerated clock skew", format!("{v}s"));
    }
    if let Some(v) = &a.revocation {
        row("revocation", format!("`{v}`"));
    }
    s.push('\n');

    // The claim names are the part that costs an afternoon when it is not
    // written down: two services reading the same token under different names
    // is a bug nobody sees until one of them is wrong about who is calling.
    let claims = [
        ("subject", &a.subject_claim),
        ("tenant", &a.tenant_claim),
        ("scopes", &a.scopes_claim),
        ("roles", &a.roles_claim),
    ];
    if claims.iter().any(|(_, v)| v.is_some()) {
        s.push_str("Claims this service reads:\n\n| meaning | claim |\n| --- | --- |\n");
        for (what, name) in claims {
            if let Some(n) = name {
                s.push_str(&format!("| {what} | `{n}` |\n"));
            }
        }
        s.push('\n');
    }
    s
}

fn methods_section(m: &Manifest) -> String {
    if m.methods.is_empty() {
        return "## What you can call\n\nNothing: this service exposes no methods. It reacts to \
                events —see below— and that is its whole surface.\n\n"
            .into();
    }
    let mut s = String::from("## What you can call\n\n| method | route | scopes | idempotent | timeout |\n| --- | --- | --- | --- | --- |\n");
    for (name, me) in &m.methods {
        let route = me.http.clone().unwrap_or_else(|| "_not over HTTP_".into());
        let scopes = if me.scopes.is_empty() {
            "—".into()
        } else {
            format!("`{}`", me.scopes.join("`, `"))
        };
        let dep = if me.deprecated.is_some() {
            " ⚠️"
        } else {
            ""
        };
        s.push_str(&format!(
            "| `{name}`{dep} | `{route}` | {scopes} | {} | {} |\n",
            if me.idempotent { "yes" } else { "no" },
            me.timeout_ms
                .map(|t| format!("{t}ms"))
                .unwrap_or_else(|| "—".into()),
        ));
    }
    s.push('\n');

    // Idempotency is the one an integrator has to act on: a method declared
    // idempotent is one they may retry, and the generated server keys off the
    // header. Saying it in a column is not enough.
    if m.methods.values().any(|me| me.idempotent) {
        s.push_str(
            "Send an `Idempotency-Key` with every idempotent call. Retrying with the same key \
             does not repeat the effect; retrying without one does.\n\n",
        );
    }

    for (name, me) in m.methods.iter().filter(|(_, me)| me.deprecated.is_some()) {
        s.push_str(&format!(
            "⚠️ `{name}` is deprecated since {}",
            me.deprecated.as_deref().unwrap_or("?")
        ));
        if let Some(d) = &me.sunset {
            s.push_str(&format!(" and stops answering on {d}"));
        }
        match &me.successor {
            Some(next) => s.push_str(&format!("; move to `{next}`.\n\n")),
            None => {
                s.push_str(". No successor is declared: ask the owner before building on it.\n\n")
            }
        }
    }

    for (name, me) in &m.methods {
        s.push_str(&format!("### `{name}`\n\n"));
        if let Some(r) = &me.http {
            s.push_str(&format!("`{r}`\n\n"));
        }
        s.push_str("Request:\n\n");
        s.push_str(&shape(&me.input));
        s.push_str("\nResponse:\n\n");
        s.push_str(&shape(&me.output));
        if !me.errors.is_empty() {
            s.push_str("\nDeclared failures, as `application/problem+json` (RFC 7807):\n\n");
            s.push_str("| status | code | retriable | meaning |\n| --- | --- | --- | --- |\n");
            for f in &me.errors {
                s.push_str(&format!(
                    "| {} | `{}` | {} | {} |\n",
                    f.status,
                    f.code,
                    if f.retriable { "yes" } else { "**no**" },
                    f.detail.as_deref().unwrap_or("—"),
                ));
            }
            s.push_str(
                "\nMatch on `code`, not on the status or the prose: the code is part of this \
                 contract and the other two are not. A failure marked not retriable will not end \
                 differently the second time.\n",
            );
        }
        s.push('\n');
    }
    s
}

fn events_section(m: &Manifest) -> String {
    let mut s = String::new();
    if !m.emits.is_empty() {
        s.push_str(
            "## Events you can subscribe to\n\nSubscribe instead of polling. Every message \
             carries the CloudEvents envelope: `traceparent`, `correlationId` and `causationId`, \
             so what you do with it stays attached to what caused it.\n\n",
        );
        if let Some(e) = &m.bus.engine {
            s.push_str(&format!("Bus: `{}`.\n\n", e.as_str()));
        }
        for (name, fields) in &m.emits {
            s.push_str(&format!("### `{name}`\n\n"));
            s.push_str(&shape(fields));
            s.push('\n');
        }
        s.push_str(
            "The version is in the name. A change that breaks you arrives as a new `@vN`, not as \
             a new field on this one.\n\n",
        );
    }
    if !m.consumes.is_empty() {
        s.push_str(
            "## Events it reacts to\n\nEmit one of these and this service acts on it — that is an \
             integration point that needs no call.\n\n",
        );
        for (name, c) in &m.consumes {
            s.push_str(&format!(
                "- `{name}` → `{}`{}\n",
                c.handler,
                c.ordered_by
                    .as_ref()
                    .map(|k| format!(", ordered by `{k}`"))
                    .unwrap_or_default(),
            ));
        }
        s.push_str(
            "\nDelivery is at least once and the consumer deduplicates by envelope id. Send the \
             same id twice and it happens once; send a new id for the same fact and it happens \
             twice.\n\n",
        );
    }
    s
}

fn streams_section(m: &Manifest) -> String {
    let mut s = String::new();
    if let Some(p) = &m.ws.path {
        s.push_str(&format!("## WebSocket\n\n`{p}`\n\n"));
        if m.ws.auth.as_deref() == Some("required") {
            s.push_str("Authenticated, ");
        }
        if !m.ws.scopes.is_empty() {
            s.push_str(&format!("scopes `{}`. ", m.ws.scopes.join("`, `")));
        }
        if let Some(h) = m.ws.heartbeat_ms {
            s.push_str(&format!("Heartbeat every {h}ms. "));
        }
        if let Some(r) = m.ws.rate_limit {
            s.push_str(&format!(
                "Rate limit {r} messages per minute PER CONNECTION —the edge sees the handshake \
                 and nothing after it, so this one is applied by the service. "
            ));
        }
        if !m.ws.origins.is_empty() {
            s.push_str(&format!(
                "Only these origins may open it: `{}`. ",
                m.ws.origins.join("`, `")
            ));
        }
        s.push_str(
            "\n\nMessages are `{ id, type, data }` and the answer comes back with the id \
                    you sent; without it, two requests in flight cannot be told apart.\n\n",
        );
        let over_ws: Vec<_> = m
            .methods
            .iter()
            .filter_map(|(n, me)| me.ws.as_ref().map(|t| (t, n)))
            .collect();
        if !over_ws.is_empty() {
            s.push_str("| type | method |\n| --- | --- |\n");
            for (t, n) in over_ws {
                s.push_str(&format!("| `{t}` | `{n}` |\n"));
            }
            s.push('\n');
        }
    }
    for (name, sse) in &m.sse {
        s.push_str(&format!("## Server-sent events: `{name}`\n\n"));
        if let Some(p) = &sse.path {
            s.push_str(&format!("`GET {p}`\n\n"));
        }
        if !sse.events.is_empty() {
            s.push_str(&format!("Carries `{}`.\n\n", sse.events.join("`, `")));
        }
        if let Some(r) = sse.retry_ms {
            s.push_str(&format!(
                "Reconnect after {r}ms; send `Last-Event-ID` and you resume where you were.\n\n"
            ));
        }
    }
    s
}

fn guarantees_section(m: &Manifest) -> String {
    let mut s = String::from("## What it guarantees\n\n");
    s.push_str(&match m.cap.consistency.as_str() {
        "strong" => "Reads are consistent. Under a partition it **rejects** rather than answer \
                     something it cannot stand behind"
            .to_string(),
        _ => "Reads are eventually consistent: what you just wrote may not be in the next read"
            .to_string(),
    });
    if m.cap.consistency != "strong" {
        s.push_str(match m.cap.on_partition.as_str() {
            "reject" => ". Under a partition it rejects",
            _ => ". Under a partition it degrades and keeps answering with what it has",
        });
    }
    if let Some(ms) = m.cap.max_staleness_ms {
        s.push_str(&format!(", up to {ms}ms stale"));
    }
    s.push_str(".\n\n");
    if !m.pii.is_empty() {
        s.push_str(&format!(
            "Personal data, declared: `{}`. It is redacted from this service's logs; what you do \
             with it on your side is your declaration to make.\n\n",
            m.pii.join("`, `")
        ));
    }
    if !m.depends.is_empty() {
        let mut deps: Vec<String> = m
            .depends
            .iter()
            .filter_map(|d| d.service.clone().or_else(|| d.external.clone()))
            .collect();
        deps.sort();
        deps.dedup();
        s.push_str(&format!(
            "It calls `{}` to do its work: a failure there shows up here.\n\n",
            deps.join("`, `")
        ));
    }
    s
}

/// What this service is held to, for whoever is deciding whether to build on
/// it. Not the matrix —that is `axon compliance`, and a control table belongs
/// in an evidence pack, not in an integration guide— but the fact that there
/// is one, and what it came back as.
fn compliance_section(m: &Manifest, pol: &crate::verify::Policy, root: &std::path::Path) -> String {
    let held = crate::compliance::in_effect(m, &pol.frameworks);
    if held.is_empty() {
        return String::new();
    }
    let cs: Vec<_> = crate::compliance::controls(m, root, &crate::compliance::declared(pol))
        .into_iter()
        .filter(|c| held.iter().any(|f| c.covers(f)))
        .collect();
    let (mut met, mut gap, mut manual) = (0, 0, 0);
    for c in &cs {
        match c.status {
            crate::compliance::Status::Met(_) => met += 1,
            crate::compliance::Status::Gap(_) => gap += 1,
            crate::compliance::Status::Manual(_) => manual += 1,
        }
    }
    let known = crate::compliance::known(pol);
    let names: Vec<&str> = known
        .iter()
        .filter(|(id, _)| held.iter().any(|f| f == id))
        .map(|(_, n)| n.as_str())
        .collect();
    let mut s = format!("## Compliance\n\nHeld to: {}.\n\n", names.join(" · "));
    s.push_str(&format!(
        "{met} control{} answered by a declaration this service is compiled from, {manual} that \
         only a person can answer",
        if met == 1 { "" } else { "s" }
    ));
    // The number nobody volunteers. Leaving it out would make this section
    // marketing, and a section that can only say good news is one a reader
    // learns to skip.
    if gap > 0 {
        s.push_str(&format!(", and **{gap} open**"));
    }
    s.push_str(
        ". The declared ones are enforced by `axon verify` on every change, so they cannot drift \
         from what is deployed without failing the build.\n\n",
    );
    s.push_str(
        "Ask for the control matrix —clause by clause, with the declaration that answers each \
         one— and it is generated from the same manifest, at any commit:\n\n\
         ```\naxon compliance <manifests>\n```\n\n",
    );
    s
}

/// The guide for one service.
fn one(m: &Manifest, pol: &crate::verify::Policy, root: &std::path::Path) -> String {
    let mut s = format!("# Integrating with `{}`\n\n", m.service);
    let mut meta: Vec<String> = Vec::new();
    if let Some(v) = &m.version {
        meta.push(format!("version {v}"));
    }
    if let Some(o) = &m.owner {
        meta.push(format!("owned by {o}"));
    }
    if let Some(t) = &m.tier {
        meta.push(format!("tier {t}"));
    }
    if !meta.is_empty() {
        s.push_str(&format!("{}\n\n", meta.join(" · ")));
    }
    s.push_str(
        "Generated by axon from the manifest. Everything below is declared and compiled into the \
         service, so it cannot drift from what actually answers.\n\n",
    );
    s.push_str(&auth_section(m));
    s.push_str(&methods_section(m));
    s.push_str(&events_section(m));
    s.push_str(&streams_section(m));
    s.push_str(&guarantees_section(m));
    s.push_str(&compliance_section(m, pol, root));
    s.push_str(
        "## The machine-readable versions\n\n- `axon openapi` — OpenAPI 3.1 for the routes above, \
         to generate a client from\n- `axon discover` — the registry as JSON\n- \
         `/.well-known/axon.json` — served by the running service, so you can check what is \
         deployed instead of what is in the repo\n",
    );
    s
}

/// The integration guide for each service, or for one of them.
pub fn build(
    ms: &[Manifest],
    pol: &crate::verify::Policy,
    root: &std::path::Path,
    only: Option<&str>,
) -> Result<String, String> {
    // `external` ones are somebody else's API, declared here only so `verify`
    // can check the calls against it. Writing their integration guide would be
    // writing documentation for a service we do not own.
    let with: Vec<&Manifest> = ms
        .iter()
        .filter(|m| !m.external)
        .filter(|m| only.is_none_or(|s| m.service == s))
        .collect();
    match (with.as_slice(), only) {
        ([], Some(s)) => Err(format!("`{s}` is not a service in these manifests")),
        ([], None) => Err("no services to document: every manifest here is `external`".into()),
        _ => Ok(with
            .iter()
            .map(|m| one(m, pol, root))
            .collect::<Vec<_>>()
            .join("\n---\n\n")),
    }
}
