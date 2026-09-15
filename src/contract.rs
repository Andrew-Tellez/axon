//! What a service's contract declares, with no language in it.
//!
//! Which types exist, what each one carries and whose schema it is: that is
//! one set of decisions, and it was written twice. The TypeScript generator
//! and the Go one both resolve a consumed event against its emitter, both
//! narrow it to the fields the consumer declared it reads, and both derive the
//! shape of an answer somebody else returns — and they had already drifted.
//! Go emitted a `…Result` for a call and TypeScript an `…Out`; Go skipped the
//! type when the caller declared no `uses` and TypeScript did not. Two names
//! for one contract is exactly what this whole tool exists to refuse.
//!
//! So the decisions live here, and a backend is what turns them into syntax.
//! The name travels in parts, without casing: `["order.placed@v1"]`,
//! `["getOrder", "In"]`, `["payments", "capturePayment", "Out"]`. Each
//! language cases and joins them the way its own reader expects —`OrderPlacedV1`
//! in both, but `OrderID` only in Go— and that is the only thing about a name
//! a language gets to decide.
use crate::manifest::{Fields, Manifest};

/// One declared type.
pub struct Decl {
    /// The name, in the parts a language cases and joins for itself.
    pub name: Vec<String>,
    /// What it is, as a sentence. Empty when the name says everything.
    pub doc: String,
    pub fields: Fields,
}

impl Decl {
    fn new(name: &[&str], doc: String, fields: Fields) -> Decl {
        Decl {
            name: name.iter().map(|s| s.to_string()).collect(),
            doc,
            fields,
        }
    }
}

/// The types of one service's contract, in three groups because a generator
/// writes them in three places: the events and the methods at the top of the
/// file, and what a call to somebody else looks like next to the client that
/// makes it.
pub struct Contract {
    /// What this service emits, and what it sees of what it consumes.
    pub events: Vec<Decl>,
    /// The input and the output of each of its methods.
    pub methods: Vec<Decl>,
    /// Per declared dependency: what it sends, and what it reads back.
    pub calls: Vec<Decl>,
}

/// Only the fields `uses` names, in the order the owner declared them.
///
/// Not a convenience: it is the rule that makes a declaration impossible to
/// contradict. The fields nobody declared reading do not exist on this side,
/// so the code cannot read one by accident and the owner can see what it is
/// free to change.
fn narrow(fields: &Fields, uses: &[String]) -> Fields {
    fields
        .iter()
        .filter(|(k, _)| uses.iter().any(|u| u == *k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

pub fn of(m: &Manifest, all: &[Manifest]) -> Result<Contract, String> {
    let mut events = Vec::new();
    for (ev, fields) in &m.emits {
        events.push(Decl::new(
            &[ev],
            format!("{ev}, emitted by this service."),
            fields.clone(),
        ));
    }
    for (ev, spec) in &m.consumes {
        // Emitted here too: the type is already above, and it is the same one.
        if m.emits.contains_key(ev) {
            continue;
        }
        // The schema of a consumed event is declared by its EMITTER, not by
        // whoever receives it. That is where the drift shows up.
        let (owner, fields) = all
            .iter()
            .find_map(|o| o.emits.get(ev).map(|f| (&o.service, f)))
            .ok_or_else(|| {
                format!(
                    "{}: consumes `{ev}` and whoever emits it was not found. Pass the other \
                     manifests: `axon build {} manifests/`",
                    m.service,
                    m.origin.display()
                )
            })?;
        let (doc, fields) = match spec.uses.as_deref() {
            Some(uses) => (
                format!(
                    "{ev}: {owner} declares {} fields; this service declared it reads {}. The \
                     rest do not exist on this side.",
                    fields.len(),
                    if uses.is_empty() {
                        "none".to_string()
                    } else {
                        uses.join(", ")
                    }
                ),
                narrow(fields, uses),
            ),
            None => (
                format!("{ev}: schema declared by {owner}, its owner."),
                fields.clone(),
            ),
        };
        events.push(Decl::new(&[ev], doc, fields));
    }

    let mut methods = Vec::new();
    for (name, me) in &m.methods {
        methods.push(Decl::new(&[name, "In"], String::new(), me.input.clone()));
        methods.push(Decl::new(&[name, "Out"], String::new(), me.output.clone()));
    }

    let mut calls = Vec::new();
    for d in &m.depends {
        let tgt = d.target();
        let other = all
            .iter()
            .find(|o| o.service == tgt)
            .ok_or_else(|| format!("{}: depends on {tgt}, which has no manifest", m.service))?;
        let sig = other
            .methods
            .get(&d.method)
            .ok_or_else(|| format!("{}: {tgt} does not expose `{}`", m.service, d.method))?;
        calls.push(Decl::new(
            &[tgt, &d.method, "In"],
            String::new(),
            sig.input.clone(),
        ));
        // The answer as THIS caller sees it: same rule as `uses` on an event.
        let (doc, fields) = match d.uses.as_deref() {
            Some(uses) => (
                format!(
                    "{tgt}.{} returns {} fields; {} declared it reads {}.",
                    d.method,
                    sig.output.len(),
                    m.service,
                    if uses.is_empty() {
                        "none".to_string()
                    } else {
                        uses.join(", ")
                    }
                ),
                narrow(&sig.output, uses),
            ),
            None => (String::new(), sig.output.clone()),
        };
        calls.push(Decl::new(&[tgt, &d.method, "Out"], doc, fields));
    }

    Ok(Contract {
        events,
        methods,
        calls,
    })
}
