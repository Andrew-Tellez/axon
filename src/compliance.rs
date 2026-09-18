//! Regimes: HIPAA, SOC 2 and whatever the team is audited against next.
//!
//! An auditor does not ask for the code. They ask which control covers a
//! requirement, where it is enforced, and how you know it did not drift. The
//! manifest already answers that for a good half of them —who may call what,
//! which fields are personal, how long a backup is kept, which fields a
//! consumer is allowed to receive— and the answer was sitting there unread.
//!
//! What this is NOT, and the report says so out loud: a certification. A
//! control this cannot see from a declaration comes back `manual` with the
//! question a human has to answer, and a regime nobody declared is not
//! checked. A tool that marked those green would be worse than no tool: it
//! would be an audit that passes without anybody looking.
//!
//! Declared in `axon.policy.toml`, next to the rest of the team's governance:
//!
//!   frameworks = ["hipaa", "soc2"]
//!
//! With that, an unmet control is an `axon verify` error like any other. Off
//! the list, `axon compliance` still reports —reading the matrix before
//! committing to it is the point.
use crate::manifest::*;
use crate::verify::{Framework, Policy};

/// What a control came back as. `Manual` is not a failure and not a pass: it
/// is the part of the regime a manifest cannot speak to.
pub enum Status {
    Met(String),
    Gap(String),
    Manual(&'static str),
}

pub struct Control {
    /// The stable name a repo's own framework points at. The title is prose
    /// and prose gets reworded; a mapping keyed on it breaks silently the day
    /// somebody fixes a typo.
    pub id: &'static str,
    /// The regimes this control answers to, each with ITS clause. One control
    /// is one property of the system; what a law calls it differs, and a
    /// matrix filtered to one regime has to print that regime's citation.
    ///
    /// Owned and not static: a framework declared in `axon.policy.toml` adds
    /// its clause to a control that already exists.
    pub cites: Vec<(String, String)>,
    pub title: &'static str,
    pub status: Status,
}

impl Control {
    pub fn covers(&self, f: &str) -> bool {
        self.cites.iter().any(|(id, _)| id == f)
    }
    /// The clauses for the regimes in play. Citing HIPAA at a service held to
    /// CFDI is citing a law that does not apply to it.
    fn clauses(&self, show: &[String]) -> String {
        self.cites
            .iter()
            .filter(|(id, _)| show.is_empty() || show.iter().any(|f| f == id))
            .map(|(_, c)| c.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

pub const FRAMEWORKS: &[(&str, &str)] = &[
    ("hipaa", "HIPAA Security Rule, 45 CFR §164"),
    ("soc2", "SOC 2 Trust Services Criteria"),
    ("iso27001", "ISO/IEC 27001:2022, Annex A"),
    (
        "lfpdppp",
        "LFPDPPP — datos personales en posesión de particulares (México)",
    ),
    (
        "cfdi",
        "CFDI 4.0 · CFF art. 28–30 — facturación electrónica (SAT)",
    ),
    ("pcidss", "PCI DSS v4.0"),
];

/// Field names that are cardholder data. Holding one of these is a decision
/// with an auditor attached, and most of the time nobody made it: it is a
/// field somebody kept because the processor returned it.
const LOOKS_LIKE_A_CARD: &[&str] = &[
    "pan",
    "cardnumber",
    "cardnum",
    "cvv",
    "cvc",
    "cvv2",
    "cav2",
    "track1",
    "track2",
    "magstripe",
    "pinblock",
    "expirymonth",
    "expiryyear",
    "cardexpiry",
];

/// Field names that are personal in every jurisdiction that has a word for it.
/// Normalized the same way the generated `redact` normalizes, so `customer_email`,
/// `customerEmail` and `customer-email` are one name.
const LOOKS_PERSONAL: &[&str] = &[
    "email",
    "phone",
    "ssn",
    "curp",
    "rfc",
    "dob",
    "birthdate",
    "address",
    "fullname",
    "lastname",
    "firstname",
    "passport",
    "ine",
    "taxid",
];

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Every field this service names anywhere: what it emits, and what its
/// methods take and return.
fn all_fields(m: &Manifest) -> Vec<&String> {
    m.emits
        .values()
        .chain(m.methods.values().map(|me| &me.input))
        .chain(m.methods.values().map(|me| &me.output))
        .flat_map(|f| f.keys())
        .collect()
}

fn list(v: &[String]) -> String {
    v.join(", ")
}

/// The controls for one service.
///
/// `custom` are the regimes this repo declared for itself. They do not add
/// controls —only a release of axon does— they add THEIR clause to a control
/// that already exists. That asymmetry is the point: a team can map its own
/// obligations onto what the compiler can really check, and cannot mark
/// something green by writing it in a config file.
pub fn controls(m: &Manifest, root: &std::path::Path, custom: &[Framework]) -> Vec<Control> {
    let mut out = Vec::new();
    let mut push = |id, cites: &[(&str, &str)], title, status| {
        out.push(Control {
            id,
            cites: cites
                .iter()
                .map(|(f, c)| ((*f).to_string(), (*c).to_string()))
                .collect(),
            title,
            status,
        })
    };

    // --- access control ---------------------------------------------------
    let unguarded: Vec<String> = m
        .methods
        .iter()
        .filter(|(_, me)| me.http.is_some())
        .filter(|(_, me)| me.auth.as_deref() != Some("required") || me.scopes.is_empty())
        .map(|(n, _)| n.clone())
        .collect();
    push(
        "access-control",
        &[
            ("hipaa", "§164.312(a)(1)"),
            ("soc2", "CC6.1"),
            ("iso27001", "A.5.15"),
            ("lfpdppp", "art. 19"),
            ("cfdi", "CFF art. 17-D"),
            ("pcidss", "req. 7.2"),
        ],
        "Every route says who may call it",
        if m.methods.values().all(|me| me.http.is_none()) {
            Status::Met("no HTTP surface: nothing to reach from outside".into())
        } else if unguarded.is_empty() {
            Status::Met(format!(
                "{n} route{s}, each with `auth = \"required\"` and declared scopes",
                n = m.methods.values().filter(|me| me.http.is_some()).count(),
                s = if m.methods.values().filter(|me| me.http.is_some()).count() == 1 {
                    ""
                } else {
                    "s"
                },
            ))
        } else {
            Status::Gap(format!(
                "no auth or no scopes: {}. A route with neither is reachable by whoever finds it",
                list(&unguarded)
            ))
        },
    );

    // --- who is calling ---------------------------------------------------
    push(
        "caller-identity",
        &[
            ("hipaa", "§164.312(a)(2)(i)"),
            ("soc2", "CC6.1"),
            ("iso27001", "A.5.16"),
            ("lfpdppp", "art. 19"),
            ("cfdi", "CFF art. 17-D"),
            ("pcidss", "req. 8.2"),
        ],
        "The caller is identified, and by a claim this service names",
        match (&m.auth.subject_claim, m.auth.issuers.is_empty()) {
            (Some(c), false) => Status::Met(format!("`{c}`, from {}", m.auth.issuers.join(", "))),
            // Nothing calls it: it reads the bus and nobody holds a token at
            // it. Demanding an issuer here would be demanding a door on a wall.
            _ if m.methods.is_empty() && m.ws.path.is_none() && m.sse.is_empty() => Status::Met(
                "no caller to identify: this service is reached only through the bus".into(),
            ),
            _ => Status::Gap(
                "`[auth]` declares no `issuers`/`subject_claim`: this service cannot say who \
                 called it, so nothing downstream can either"
                    .into(),
            ),
        },
    );

    // --- token lifetime ---------------------------------------------------
    push(
        "session-expiry",
        &[
            ("hipaa", "§164.312(a)(2)(iii)"),
            ("soc2", "CC6.1"),
            ("iso27001", "A.8.5"),
            ("pcidss", "req. 8.6"),
        ],
        "A credential stops working on its own",
        match m.auth.max_token_age_s {
            Some(s) => Status::Met(format!(
                "`max_token_age_s = {s}`, revocation `{}`",
                m.auth.revocation.as_deref().unwrap_or("not declared")
            )),
            None if m.auth.issuers.is_empty() => Status::Manual(
                "this service declares no `[auth]`: whoever runs the gateway has to show the \
                 token lifetime it enforces",
            ),
            None => Status::Gap(
                "`[auth]` declares no `max_token_age_s`: a token stolen today is a token that \
                 works until its own `exp`, and nothing here bounds that"
                    .into(),
            ),
        },
    );

    // --- tenant isolation -------------------------------------------------
    if m.infra.state.is_some() {
        push(
            "tenant-isolation",
            &[
                ("hipaa", "§164.308(a)(4)"),
                ("soc2", "CC6.1"),
                ("iso27001", "A.5.15"),
                ("lfpdppp", "art. 19"),
                ("cfdi", "CFF art. 28"),
                ("pcidss", "req. 7.2"),
            ],
            "One tenant's rows cannot be read by another",
            match &m.infra.tenant_column {
                Some(c) => Status::Met(format!(
                    "`{c}` on every table, enforced by row-level security (`axon rls`)"
                )),
                None => Status::Gap(
                    "`[infra] tenant_column` is not declared, so no RLS policy is generated and \
                     isolation is left to whoever writes the next WHERE"
                        .into(),
                ),
            },
        );
    }

    // --- personal data, declared -----------------------------------------
    let declared: Vec<String> = m.pii.iter().map(|p| norm(p)).collect();
    let missed: Vec<String> = all_fields(m)
        .iter()
        .filter(|f| {
            let n = norm(f);
            LOOKS_PERSONAL.iter().any(|p| n.contains(p)) && !declared.contains(&n)
        })
        .map(|f| (*f).clone())
        .collect();
    push(
        "data-inventory",
        &[
            ("hipaa", "§164.514"),
            ("soc2", "CC6.7"),
            ("iso27001", "A.5.12"),
            ("lfpdppp", "art. 20"),
            ("pcidss", "req. 3.3"),
        ],
        "Personal data is declared, so it can be redacted and masked",
        if !missed.is_empty() {
            Status::Gap(format!(
                "these read as personal and are not in `pii`: {}. Undeclared means unredacted in \
                 the logs and unmasked in the warehouse",
                list(&missed)
            ))
        } else if m.pii.is_empty() {
            Status::Met("no field here reads as personal data".into())
        } else {
            Status::Met(format!(
                "`pii = [{}]` — redacted from logs by the generated `redact`",
                list(&m.pii)
            ))
        },
    );

    // --- minimum necessary ------------------------------------------------
    let greedy: Vec<String> = m
        .consumes
        .iter()
        .filter(|(_, c)| c.uses.is_none())
        .map(|(n, _)| n.clone())
        .chain(m.depends.iter().filter(|d| d.uses.is_none()).map(|d| {
            format!(
                "{}.{}",
                d.service
                    .as_deref()
                    .or(d.external.as_deref())
                    .unwrap_or("?"),
                d.method
            )
        }))
        .collect();
    if !m.consumes.is_empty() || !m.depends.is_empty() {
        push(
            "minimum-necessary",
            &[
                ("hipaa", "§164.502(b)"),
                ("soc2", "CC6.7"),
                ("iso27001", "A.5.14"),
                ("lfpdppp", "art. 13"),
                ("pcidss", "req. 7.2.1"),
            ],
            "It receives only the fields it declared it needs",
            if greedy.is_empty() {
                Status::Met(
                    "every `consumes` and `depends` declares `uses`; the generated types carry \
                     nothing else, so a field nobody asked for does not compile"
                        .into(),
                )
            } else {
                Status::Gap(format!(
                    "no `uses` on: {}. Without it the whole payload arrives, including what this \
                     service has no business holding",
                    list(&greedy)
                ))
            },
        );
    }

    // --- personal data on a public surface --------------------------------
    let public_leak: Vec<String> = m
        .sse
        .iter()
        .filter(|(_, s)| s.auth.as_deref() == Some("public"))
        .filter(|(_, s)| {
            s.events.iter().any(|e| {
                m.emits
                    .get(e)
                    .is_some_and(|f| f.keys().any(|k| declared.contains(&norm(k))))
            })
        })
        .map(|(n, _)| n.clone())
        .collect();
    push(
        "transmission-scope",
        &[
            ("hipaa", "§164.312(e)(1)"),
            ("soc2", "CC6.6"),
            ("iso27001", "A.8.12"),
            ("lfpdppp", "art. 19"),
            ("cfdi", "CFF art. 28"),
            ("pcidss", "req. 4.2"),
        ],
        "Personal data does not leave through an unauthenticated surface",
        if public_leak.is_empty() {
            Status::Met("no public stream carries a field declared as personal".into())
        } else {
            Status::Gap(format!(
                "public streams carrying declared personal data: {}",
                list(&public_leak)
            ))
        },
    );

    // --- contingency ------------------------------------------------------
    if m.infra.state.is_some() {
        let tier_critical = matches!(m.tier.as_deref(), Some("0") | Some("1"));
        push(
            "backup",
            &[
                ("hipaa", "§164.308(a)(7)(ii)(A)"),
                ("soc2", "A1.2"),
                ("iso27001", "A.8.13"),
                ("cfdi", "CFF art. 30"),
                ("pcidss", "req. 12.10"),
            ],
            "The data survives losing the database",
            match (m.infra.backup_retention_days, m.infra.pitr) {
                (Some(d), Some(true)) => {
                    Status::Met(format!("{d} days of backups, point-in-time recovery on"))
                }
                (Some(d), _) if !tier_critical => Status::Met(format!("{d} days of backups")),
                (Some(d), _) => Status::Gap(format!(
                    "{d} days of backups but no `pitr` on a tier {} service: recovery lands on \
                     the last snapshot, and everything after it is gone",
                    m.tier.as_deref().unwrap_or("?")
                )),
                (None, _) => Status::Gap(
                    "`[infra] backup_retention_days` is not declared: nothing says a backup is \
                     taken, or for how long it is kept"
                        .into(),
                ),
            },
        );
    }

    // --- retention --------------------------------------------------------
    if m.analytics.export {
        push(
            "retention",
            &[
                ("hipaa", "§164.316(b)(2)(i)"),
                ("soc2", "CC6.5"),
                ("iso27001", "A.5.33"),
                ("lfpdppp", "art. 11"),
                ("pcidss", "req. 3.2.1"),
            ],
            "Exported data has an end date",
            match m.analytics.retention_days {
                Some(d) => Status::Met(format!(
                    "{d} days in the warehouse, personal data `{}`",
                    m.analytics.pii
                )),
                None => Status::Gap(
                    "`[analytics] export = true` with no `retention_days`: what lands in the \
                     warehouse stays there forever, including what somebody asked to be deleted"
                        .into(),
                ),
            },
        );
    }

    // --- change management ------------------------------------------------
    push(
        "change-management",
        &[
            ("soc2", "CC8.1"),
            ("iso27001", "A.8.32"),
            ("pcidss", "req. 6.3"),
        ],
        "A breaking change to a published contract cannot ship unnoticed",
        if root.join(crate::baseline::ARCHIVO).exists() {
            Status::Met(format!(
                "`{}` is in the repo; `axon verify` refuses an incompatible change against it",
                crate::baseline::ARCHIVO
            ))
        } else {
            Status::Gap(format!(
                "no `{}`: nothing compares what is about to ship against what is published. \
                 Generate it with `axon baseline`",
                crate::baseline::ARCHIVO
            ))
        },
    );

    // --- a fiscal document is issued once ---------------------------------
    let repeatable: Vec<String> = m
        .methods
        .iter()
        .filter(|(_, me)| {
            me.http
                .as_deref()
                .is_some_and(|h| h.starts_with("POST") || h.starts_with("PUT"))
        })
        .filter(|(_, me)| !me.idempotent)
        .map(|(n, _)| n.clone())
        .collect();
    push(
        "issue-once",
        &[("cfdi", "CFF art. 29")],
        "Issuing twice does not issue two documents",
        if repeatable.is_empty() {
            Status::Met(
                "every mutating route is `idempotent`, so the generated client sends an `Idempotency-Key` and a retry lands on the same document"
                    .into(),
            )
        } else {
            Status::Gap(format!(
                "not idempotent: {}. A timeout the caller retries is a second folio fiscal for one operation, and cancelling it is paperwork somebody has to do by hand",
                list(&repeatable)
            ))
        },
    );

    // --- the fiscal archive -----------------------------------------------
    if !m.infra.buckets.is_empty() {
        // Five years, counted from the day the return is filed (CFF art. 30).
        const FIVE_YEARS: u32 = 1826;
        let short: Vec<String> = m
            .infra
            .buckets
            .iter()
            .filter(|(_, b)| b.retention_days.is_none_or(|d| d < FIVE_YEARS))
            .map(|(n, b)| match b.retention_days {
                Some(d) => format!("{n} ({d}d)"),
                None => format!("{n} (no retention declared)"),
            })
            .collect();
        push(
            "archive-retention",
            &[("cfdi", "CFF art. 30")],
            "The archive outlives the audit window",
            if short.is_empty() {
                Status::Met(format!("every bucket keeps at least {FIVE_YEARS} days"))
            } else {
                Status::Gap(format!(
                    "under five years: {}. The SAT can ask for a document issued five years ago and a lifecycle rule does not care that it was fiscal",
                    list(&short)
                ))
            },
        );

        let public: Vec<String> = m
            .infra
            .buckets
            .iter()
            .filter(|(_, b)| b.public)
            .map(|(n, _)| n.clone())
            .collect();
        push(
            "archive-private",
            &[("cfdi", "CFF art. 28"), ("lfpdppp", "art. 19")],
            "The archive is not readable by whoever guesses the URL",
            if public.is_empty() {
                Status::Met("no bucket is public".into())
            } else {
                Status::Gap(format!(
                    "public buckets: {}. A document carrying an RFC and an address behind an unauthenticated link is not access control, it is obscurity —serve it with a signed URL instead",
                    list(&public)
                ))
            },
        );
    }

    // --- cancelled, not edited --------------------------------------------
    push(
        "document-immutable",
        &[("cfdi", "CFF art. 29-A")],
        "A document is cancelled, never rewritten",
        if m.machine.values().any(|mc| !mc.final_states.is_empty()) {
            Status::Met(format!(
                "`[machine.*]` declares final states ({}); the generated transition refuses an illegal one instead of leaving it to the next UPDATE",
                m.machine
                    .values()
                    .flat_map(|mc| mc.final_states.iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        } else {
            Status::Gap(
                "no `[machine.*]` with final states: nothing stops a issued document from moving back, and a fiscal document that can be edited is one the SAT reads as a second one"
                    .into(),
            )
        },
    );

    // --- the signing key --------------------------------------------------
    push(
        "key-custody",
        &[("cfdi", "CFF art. 17-D")],
        "The signing key comes from a store, not from the image",
        if !m.infra.secrets.is_empty() {
            Status::Met(format!(
                "declared secrets: {} — mounted by reference (`secretKeyRef`), never written into the manifest or the image",
                list(&m.infra.secrets)
            ))
        } else {
            Status::Gap(
                "`[infra] secrets` is empty: nothing says where the CSD private key comes from, and a key nobody declared is a key somebody put in an env var"
                    .into(),
            )
        },
    );

    // --- cardholder data ---------------------------------------------------
    let cards: Vec<String> = all_fields(m)
        .iter()
        .filter(|f| {
            let n = norm(f);
            LOOKS_LIKE_A_CARD.iter().any(|c| n == *c || n.ends_with(c))
        })
        .map(|f| (*f).clone())
        .collect();
    push(
        "no-cardholder-data",
        &[("pcidss", "req. 3.2 / 3.3")],
        "Cardholder data is not here at all",
        if cards.is_empty() {
            Status::Met(
                "no field reads as a PAN, a CVV or track data. A service that never holds them \
                 is a service outside the scope of the assessment, which is cheaper than any \
                 control over them"
                    .into(),
            )
        } else {
            Status::Gap(format!(
                "these read as cardholder data: {}. A CVV may not be stored after authorization \
                 at all, and a PAN pulls this whole service into scope —take a token from the \
                 processor and declare that instead",
                list(&cards)
            ))
        },
    );

    // --- ARCO --------------------------------------------------------------
    push(
        "data-subject-rights",
        &[("lfpdppp", "arts. 22-34")],
        "The holder can reach, correct and erase their data",
        Status::Manual(
            "the declared half is here —`pii` says which fields, `retention_days` says how long— but who answers an ARCO request, in what window and through which route is a process, and a manifest cannot hold a process. Write it in the privacy notice and name the method that serves it",
        ),
    );

    // --- the ones a manifest cannot see -----------------------------------
    push(
        "encryption-at-rest",
        &[
            ("hipaa", "§164.312(a)(2)(iv)"),
            ("soc2", "CC6.1"),
            ("iso27001", "A.8.24"),
            ("lfpdppp", "art. 19"),
            ("cfdi", "CFF art. 28"),
            ("pcidss", "req. 3.5"),
        ],
        "Stored data is encrypted",
        Status::Manual("a property of the database you provision; show the provider's setting"),
    );
    push(
        "encryption-in-transit",
        &[
            ("hipaa", "§164.312(e)(2)(ii)"),
            ("soc2", "CC6.6"),
            ("iso27001", "A.8.24"),
            ("lfpdppp", "art. 19"),
            ("cfdi", "CFF art. 28"),
            ("pcidss", "req. 4.2"),
        ],
        "TLS on every hop",
        Status::Manual("terminated at the edge, outside the manifest; show the gateway's config"),
    );
    push(
        "audit-controls",
        &[
            ("hipaa", "§164.312(b)"),
            ("soc2", "CC7.2"),
            ("iso27001", "A.8.15"),
            ("lfpdppp", "art. 19"),
            ("cfdi", "CFF art. 28"),
            ("pcidss", "req. 10.2"),
        ],
        "Access to personal data is recorded",
        Status::Manual(
            "the envelope carries `correlationId` and `causationId` through every hop, which is \
             what makes an access log reconstructable —but axon does not store it. Show where \
             those logs land and how long they are kept",
        ),
    );
    push(
        "workforce-training",
        &[
            ("hipaa", "§164.308(a)(5)"),
            ("soc2", "CC1.4"),
            ("iso27001", "A.6.3"),
            ("pcidss", "req. 12.6"),
        ],
        "The people have been trained",
        Status::Manual("nothing in a repository can answer this"),
    );

    // The repo's own regimes, mapped onto the controls that already exist.
    for f in custom {
        for c in out.iter_mut() {
            if let Some(clause) = f.controls.get(c.id) {
                c.cites.push((f.id.clone(), clause.clone()));
            }
        }
    }
    out.retain(|c| !c.cites.is_empty());
    out
}

/// The unmet controls, as `verify` findings. Only for the declared frameworks:
/// a regime nobody signed up for is not a rule, it is a suggestion.
pub fn findings(ms: &[Manifest], pol: &Policy, root: &std::path::Path) -> Vec<String> {
    let custom = declared(pol);
    let mut out = Vec::new();
    // A mapping with a typo is worse than no mapping: it reads as covered.
    let ids = ids();
    for f in &custom {
        for key in f.controls.keys() {
            if !ids.contains(&key.as_str()) {
                out.push(format!(
                    "[framework.{}] maps `{key}`, which is not a control. `axon compliance --ids` \
                     lists them",
                    f.id
                ));
            }
        }
    }
    for m in ms.iter().filter(|m| !m.external) {
        // A regime a service does not carry is not a rule for it. Naming one
        // axon does not know is its own error, below.
        for name in &m.compliance {
            if !FRAMEWORKS.iter().any(|(id, _)| id == name) {
                out.push(format!(
                    "{}: `compliance = [\"{name}\"]` is not a regime axon knows. These are: {}",
                    m.service,
                    FRAMEWORKS
                        .iter()
                        .map(|(id, _)| *id)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        let held = in_effect(m, &pol.frameworks);
        for c in controls(m, root, &custom) {
            let wanted = held.iter().any(|f| c.covers(f));
            if let (true, Status::Gap(why)) = (wanted, &c.status) {
                out.push(format!(
                    "{}: [{}] {} — {why}",
                    m.service,
                    c.clauses(&held),
                    c.title.to_lowercase()
                ));
            }
        }
    }
    out
}

/// Every control id, which is the surface a repo's own framework maps onto.
///
/// Evaluated rather than listed: a hand-written list would drift from the
/// controls the day somebody adds one, and a mapping pointing at an id that
/// no longer exists is a control a team believes is covered. The reference
/// manifest exists only to trip every gate.
pub fn ids() -> Vec<&'static str> {
    const REFERENCE: &str = r#"
service = "reference"

[infra]
state = "postgres"

[infra.buckets.archive]

[analytics]
export = true

[consumes."x@v1"]
handler = "onX"
"#;
    let m: Manifest = toml::from_str(REFERENCE).expect("the reference manifest has to parse");
    controls(&m, std::path::Path::new("."), &[])
        .into_iter()
        .map(|c| c.id)
        .collect()
}

/// The repo's own regimes, with their ids filled in from the block names.
pub fn declared(pol: &Policy) -> Vec<Framework> {
    pol.framework
        .iter()
        .map(|(id, f)| Framework {
            id: id.clone(),
            ..f.clone()
        })
        .collect()
}

/// Every regime this axon can name: the ones it ships with, and the ones the
/// repo declared.
pub fn known(pol: &Policy) -> Vec<(String, String)> {
    FRAMEWORKS
        .iter()
        .map(|(id, n)| ((*id).to_string(), (*n).to_string()))
        .chain(
            pol.framework
                .iter()
                .map(|(id, f)| (id.clone(), f.name.clone())),
        )
        .collect()
}

/// The regimes a service answers to: the repo's, plus its own.
pub fn in_effect(m: &Manifest, frameworks: &[String]) -> Vec<String> {
    let mut all: Vec<String> = frameworks.to_vec();
    for f in &m.compliance {
        if !all.contains(f) {
            all.push(f.clone());
        }
    }
    all
}

/// The control matrix: what an auditor asks for, and the declaration that
/// answers it.
pub fn build(
    ms: &[Manifest],
    root: &std::path::Path,
    pol: &Policy,
    only: Option<&str>,
) -> Result<String, String> {
    let custom = declared(pol);
    let frameworks = &pol.frameworks;
    let known = known(pol);
    if let Some(f) = only {
        if !known.iter().any(|(id, _)| id == f) {
            return Err(format!(
                "`{f}` is not a regime this repo knows. These are: {}",
                known
                    .iter()
                    .map(|(id, _)| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    let mut s = String::from("# Control matrix\n\n");
    s.push_str(&format!(
        "{}\n\n",
        match only {
            // Asking about one regime is asking what it WOULD demand, so it is
            // answered for every service whether or not it carries it today.
            Some(f) => format!(
                "{} — every service, held to this regime whether or not it declares it",
                known.iter().find(|(id, _)| id == f).unwrap().1
            ),
            None => "Each service against the regimes in effect for it: the repo's \
                     `frameworks`, plus its own `compliance`."
                .to_string(),
        }
    ));

    s.push_str(
        "Generated from the manifests. **This is not a certification.** It is the subset of each \
         regime that a declaration can answer, plus the questions it cannot —those come back \
         `manual`, with what a person has to show. A control marked `met` means the declaration \
         is in place and `axon verify` keeps it there; it does not mean somebody audited the \
         running system.\n\n",
    );
    if only.is_none_or(|f| f == "lfpdppp" || f == "cfdi") {
        s.push_str(
            "The clauses of a law are cited as a map, not as advice: which article really binds this service is your counsel's call, and the citation is here so the conversation starts from something concrete.\n\n",
        );
    }

    let mut gaps = 0;
    for m in ms.iter().filter(|m| !m.external) {
        let held = in_effect(m, frameworks);
        let cs: Vec<Control> = controls(m, root, &custom)
            .into_iter()
            .filter(|c| match only {
                Some(f) => c.covers(f),
                None => held.iter().any(|f| c.covers(f)),
            })
            .collect();
        s.push_str(&format!("## `{}`\n\n", m.service));
        if cs.is_empty() {
            // Said out loud, not skipped. A service missing from the matrix
            // reads as an oversight; one that says it is in no regime's scope
            // is a decision somebody can disagree with.
            s.push_str(
                "No regime in effect: nothing in `axon.policy.toml` and no `compliance` on this \
                 manifest. Nothing is checked, and nothing here says it was.\n\n",
            );
            continue;
        }
        if only.is_none() {
            s.push_str(&format!("In effect: `{}`\n\n", held.join("`, `")));
        }
        s.push_str("| control | clause | status | evidence |\n| --- | --- | --- | --- |\n");
        for c in &cs {
            let (mark, why) = match &c.status {
                Status::Met(e) => ("met", e.clone()),
                Status::Gap(e) => {
                    gaps += 1;
                    ("**GAP**", e.clone())
                }
                Status::Manual(e) => ("manual", (*e).to_string()),
            };
            s.push_str(&format!(
                "| {} | {} | {mark} | {why} |\n",
                c.title,
                c.clauses(&match only {
                    Some(f) => vec![f.to_string()],
                    None => held.clone(),
                })
            ));
        }
        s.push('\n');
    }
    s.push_str(&format!(
        "---\n\n{gaps} gap(s){}\n",
        // Suggesting a regime already in the policy reads as if it were not
        // applying, which is the one thing the reader has to be sure about.
        match only {
            Some(f) if !frameworks.contains(&f.to_string()) => format!(
                ". `{f}` is not in `axon.policy.toml`, so none of this is enforced yet: add it to \
                 `frameworks` —or to this service's `compliance`— and every gap above becomes an \
                 `axon verify` error"
            ),
            _ if frameworks.is_empty() => ". Nothing is declared in `axon.policy.toml`, so none \
                 of this is enforced: a matrix somebody reads once is not a rule that fails the \
                 build"
                .to_string(),
            _ => format!(
                ", enforced by `axon verify` on every change: `{}` {} declared in the policy",
                frameworks.join("`, `"),
                if frameworks.len() == 1 { "is" } else { "are" }
            ),
        }
    ));
    Ok(s)
}
