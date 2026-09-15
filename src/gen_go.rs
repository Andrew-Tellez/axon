//! The Go generator, native.
//!
//! It was a plugin, and being a plugin was the point for a while: it proved
//! the protocol holds a real generator. What it could not prove is the claim
//! underneath the whole project —that the manifest is not TypeScript in
//! disguise— because nobody runs a generator they have to build first.
//!
//! It produces idiomatic Go and not translated TypeScript: an interface the
//! person implements instead of inheritance, `ctx` first and `error` last,
//! `OrderID` and not `OrderId`. The output is already `gofmt`-clean, checked
//! with the real `gofmt` and `go vet`: a generator should not leave code
//! somebody has to format afterwards.
use crate::manifest::*;

/// Go's own initialisms. `orderId` is written `OrderID`, and a generator that
/// gets this wrong produces code that reads as foreign in its own language.
const INITIALISMS: [&str; 8] = ["id", "url", "api", "http", "json", "sql", "uri", "uuid"];

fn exported(s: &str) -> String {
    s.split(['.', '@', '_', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let up = w.to_uppercase();
            if INITIALISMS.contains(&w.to_lowercase().as_str()) {
                return up;
            }
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// A field's name, with the trailing initialism Go expects: `orderId` is
/// `OrderID`.
fn field(s: &str) -> String {
    let out = exported(s);
    for suf in ["Id", "Url", "Api", "Uri"] {
        if out.ends_with(suf) && out.len() > suf.len() {
            return format!("{}{}", &out[..out.len() - suf.len()], suf.to_uppercase());
        }
    }
    out
}

/// gofmt lines up consecutive lines that have the same shape. Emitting it
/// already aligned is not cosmetics: the output has to be formatted, not
/// formattable, and `gofmt -l` in the suite is what says whether it is.
fn aligned(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let width = |i: usize| {
        rows.iter()
            .filter_map(|r| r.get(i))
            .map(|c| c.chars().count())
            .max()
            .unwrap_or(0)
    };
    let widths: Vec<usize> = (0..cols).map(width).collect();
    rows.iter()
        .map(|r| {
            let mut line = String::from("\t");
            for (i, c) in r.iter().enumerate() {
                match i + 1 == r.len() {
                    true => line.push_str(c),
                    false => line.push_str(&format!(
                        "{c}{} ",
                        " ".repeat(widths[i].saturating_sub(c.chars().count()))
                    )),
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

fn go_type(t: &str) -> &str {
    match t {
        "int" => "int64",
        "float" => "float64",
        "bool" => "bool",
        "timestamp" => "time.Time",
        "json" => "json.RawMessage",
        "money" => "Money",
        _ => "string",
    }
}

/// A doc comment, wrapped. gofmt does not wrap comments, so a generator that
/// emits one long line leaves a long line in somebody's editor forever.
/// Wrapped at 78 columns, one paragraph at a time.
///
/// The break between paragraphs is not decoration here: Go reads `Deprecated:`
/// only when it opens one, so a deprecation folded into the line above it is a
/// deprecation no linter and no editor will ever mention.
fn comment(text: &str) -> String {
    let mut out = String::new();
    for (i, para) in text.split("\n\n").enumerate() {
        if i > 0 {
            out.push_str("//\n");
        }
        let mut line = String::from("//");
        for w in para.split_whitespace() {
            if line.len() + 1 + w.len() > 78 {
                out.push_str(&format!("{line}\n"));
                line = String::from("//");
            }
            line.push(' ');
            line.push_str(w);
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// A struct, with the columns lined up the way `gofmt` would leave them: the
/// output has to be formatted already, not formattable.
fn struct_of(name: &str, fields: &[(&String, &String)], doc: &str) -> String {
    let mut o = String::new();
    if !doc.is_empty() {
        o.push_str(&comment(doc));
    }
    if fields.is_empty() {
        return o + &format!("type {name} struct{{}}\n\n");
    }
    let rows: Vec<Vec<String>> = fields
        .iter()
        .map(|(n, t)| vec![field(n), go_type(t).to_string(), format!("`json:\"{n}\"`")])
        .collect();
    o.push_str(&format!("type {name} struct {{\n{}}}\n\n", aligned(&rows)));
    o
}

/// `errors` only when something declares a failure: an unused import does not
/// compile in Go, which is the language saying the generator lied about what
/// it needs.
/// The import block, read off the body. An unused import does not compile in
/// Go —which is the language saying the generator lied about what it needs—
/// and a missing one does not either, so neither side can be a flag somebody
/// forgets to pass.
fn imports(body: &str) -> String {
    let mut out = vec![
        "context",
        "crypto/rand",
        "encoding/hex",
        "encoding/json",
        "fmt",
        "strings",
        "time",
    ];
    for (pkg, used) in [
        ("encoding/binary", "binary."),
        ("errors", "errors."),
        ("sync", "sync."),
    ] {
        if body.contains(used) {
            out.push(pkg);
        }
    }
    // gofmt keeps one block sorted
    out.sort_unstable();
    out.iter().map(|i| format!("\t{i:?}\n")).collect::<String>()
}

fn header(pkg: &str, body: &str) -> String {
    let imports = imports(body);
    format!(
        r#"// Code generated by axon. DO NOT EDIT.

package {pkg}

import (
{imports})

// Money is its own type on purpose: a float for money is a bug waiting its
// turn.
type Money struct {{
	Amount   int64  `json:"amount"`
	Currency string `json:"currency"`
}}

// Envelope is CloudEvents plus the causal chain. Traceability is not optional.
type Envelope struct {{
	ID            string          `json:"id"`
	Type          string          `json:"type"`
	Source        string          `json:"source"`
	Time          string          `json:"time"`
	Traceparent   string          `json:"traceparent"`
	CorrelationID string          `json:"correlationId"`
	CausationID   *string         `json:"causationId"`
	Data          json.RawMessage `json:"data"`
}}

type Bus interface {{
	Publish(ctx context.Context, e Envelope) error
}}

// Outbox: the event is saved in the same transaction as the state change.
type Outbox interface {{
	Stage(ctx context.Context, e Envelope) error
}}

// Inbox: the broker delivers at least once, the effect happens once.
type Inbox interface {{
	Once(ctx context.Context, id string, fn func(context.Context) error) error
}}

func randomHex(n int) string {{
	b := make([]byte, n)
	if _, err := rand.Read(b); err != nil {{
		panic(err)
	}}
	return hex.EncodeToString(b)
}}

func uuid4() string {{
	b := make([]byte, 16)
	if _, err := rand.Read(b); err != nil {{
		panic(err)
	}}
	b[6] = (b[6] & 0x0f) | 0x40
	b[8] = (b[8] & 0x3f) | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}}

// NewEnvelope carries the causal chain forward: the trace is inherited, the
// span is new, and causationId points at the message that caused this one.
func NewEnvelope(kind, source string, data any, cause *Envelope) (Envelope, error) {{
	payload, err := json.Marshal(data)
	if err != nil {{
		return Envelope{{}}, err
	}}
	trace, correlation := randomHex(16), uuid4()
	var causation *string
	if cause != nil {{
		if parts := strings.SplitN(cause.Traceparent, "-", 4); len(parts) >= 2 {{
			trace = parts[1]
		}}
		correlation = cause.CorrelationID
		id := cause.ID
		causation = &id
	}}
	return Envelope{{
		ID:            uuid4(),
		Type:          kind,
		Source:        source,
		Time:          time.Now().UTC().Format(time.RFC3339Nano),
		Traceparent:   fmt.Sprintf("00-%s-%s-01", trace, randomHex(8)),
		CorrelationID: correlation,
		CausationID:   causation,
		Data:          payload,
	}}, nil
}}

"#
    )
}

/// The `Problem` type itself. Both the declared failures and the generated
/// `RequireScopes` return one, so it is emitted when either exists: a
/// manifest with scopes and no declared errors used to generate code that
/// did not compile.
fn problem(m: &Manifest) -> String {
    // Whoever CALLS needs it too: a client asks a failure for its code
    // before deciding whether trying again is worth anything.
    let needed = !m.depends.is_empty()
        || m.methods
            .values()
            .any(|me| !me.errors.is_empty() || !me.scopes.is_empty());
    if !needed {
        return String::new();
    }
    String::from(
        "// Problem is what travels on the wire when something declared fails:\n\
         // RFC 7807, with the code the manifest declares.\n\
         type Problem struct {\n\
         \tStatus    int    `json:\"status\"`\n\
         \tCode      string `json:\"title\"`\n\
         \tDetail    string `json:\"detail\"`\n\
         \tRetriable bool   `json:\"-\"`\n\
         }\n\n\
         func (p *Problem) Error() string {\n\
         \tif p.Detail == \"\" {\n\
         \t\treturn p.Code\n\
         \t}\n\
         \treturn p.Code + \": \" + p.Detail\n\
         }\n\n\
         // Retriable answers the only question a caller has about a failure it\n\
         // did not expect: is trying again worth anything.\n\
         func Retriable(err error) bool {\n\
         \tvar p *Problem\n\
         \tif errors.As(err, &p) {\n\
         \t\treturn p.Retriable\n\
         \t}\n\
         \treturn false\n\
         }\n\n",
    )
}

/// RFC 7807 and the declared failures. In TypeScript an undeclared code does
/// not compile because the type is a union of literals; Go has no such type,
/// so the contract is a named string type with one constant per declared
/// failure. A conversion can still bypass it — what cannot drift is the
/// status and the `retriable` that come out of the manifest.
fn failures(m: &Manifest) -> String {
    let with: Vec<(&String, &Method)> = m
        .methods
        .iter()
        .filter(|(_, me)| !me.errors.is_empty())
        .collect();
    if with.is_empty() {
        return String::new();
    }
    let mut o = String::new();
    for (name, me) in with {
        let t = format!("{}Error", exported(name));
        o.push_str(&format!(
            "// The failures {} declares. A method that fails in a way nobody\n\
             // declared is a 500 with no code, which is the honest answer.\n\
             type {t} string\n\n\
             const (\n",
            camel(name)
        ));
        let consts: Vec<Vec<String>> = me
            .errors
            .iter()
            .map(|f| {
                vec![
                    format!("{}{}", exported(name), exported(&f.code)),
                    t.clone(),
                    format!("= {:?}", f.code),
                ]
            })
            .collect();
        o.push_str(&aligned(&consts));
        o.push_str(")\n\n");
        let rows: Vec<Vec<String>> = me
            .errors
            .iter()
            .map(|f| {
                vec![
                    format!("{:?}:", f.code),
                    format!(
                        "{{Status: {}, Code: {:?}, Detail: {:?}, Retriable: {}}},",
                        f.status,
                        f.code,
                        f.detail.clone().unwrap_or_default(),
                        f.retriable
                    ),
                ]
            })
            .collect();
        o.push_str(&format!(
            "var {}Failures = map[{t}]Problem{{\n{}}}\n\n",
            exported(name),
            aligned(&rows)
        ));
        o.push_str(&format!(
            "// Fail returns the declared failure, with its status and its\n\
             // retriable straight out of the manifest.\n\
             func (c {t}) Fail(detail string) error {{\n\
             \tp := {}Failures[c]\n\
             \tif detail != \"\" {{\n\
             \t\tp.Detail = detail\n\
             \t}}\n\
             \treturn &p\n\
             }}\n\n",
            exported(name)
        ));
    }
    o
}

/// The scopes a method demands. Declared in the manifest and not in an `if`
/// inside a handler, which is the version nobody can audit from outside.
fn scopes(m: &Manifest) -> String {
    let with: Vec<(&String, &Method)> = m
        .methods
        .iter()
        .filter(|(_, me)| !me.scopes.is_empty())
        .collect();
    if with.is_empty() {
        return String::new();
    }
    let mut o = String::from(
        "// RequireScopes answers with RFC 6750's `insufficient_scope`, and not\n\
         // with a bare 403: a caller has to be able to tell \"you are nobody\"\n\
         // from \"you are somebody who may not do this\".\n\
         func RequireScopes(granted []string, need ...string) error {\n\
         \tfor _, want := range need {\n\
         \t\tfound := false\n\
         \t\tfor _, g := range granted {\n\
         \t\t\tif g == want {\n\
         \t\t\t\tfound = true\n\
         \t\t\t\tbreak\n\
         \t\t\t}\n\
         \t\t}\n\
         \t\tif !found {\n\
         \t\t\treturn &Problem{Status: 403, Code: \"insufficient_scope\", Detail: \"requires \" + want}\n\
         \t\t}\n\
         \t}\n\
         \treturn nil\n\
         }\n\n",
    );
    for (name, me) in with {
        let list: Vec<String> = me.scopes.iter().map(|s| format!("{s:?}")).collect();
        o.push_str(&format!(
            "// {} requires {}.\nvar {}Scopes = []string{{{}}}\n\n",
            camel(name),
            me.scopes.join(", "),
            exported(name),
            list.join(", ")
        ));
    }
    o
}

/// The types of the events and methods, with one rule that is the whole point
/// of `uses`: a consumed event's type carries ONLY the fields this service
/// declared it reads. The others do not exist on this side, so the
/// declaration cannot drift from the code.
/// A declared name as Go writes it.
fn go_name(d: &crate::contract::Decl) -> String {
    d.name.iter().map(|p| exported(p)).collect()
}

fn types(c: &crate::contract::Contract) -> String {
    let llamadas: Vec<&crate::contract::Decl> =
        c.calls.iter().flat_map(|c| [&c.input, &c.output]).collect();
    let mut o = String::new();
    for d in c.events.iter().chain(&c.methods).chain(llamadas) {
        o.push_str(&struct_of(
            &go_name(d),
            &d.fields.iter().collect::<Vec<_>>(),
            &d.doc,
        ));
    }
    o
}

/// The resilience the manifest declared, as Go runs it.
///
/// Same contract as the TypeScript one and not the same code: there the
/// timeout is a promise racing another, here it is the context the call
/// already takes, and the breaker is a map behind a mutex instead of a
/// closure. What is identical is what was declared —the numbers, which
/// failures are worth another try, what happens when the other side is
/// unreachable— because that comes from `contract` and not from either
/// generator.
const RESILIENCE_GO: &str = r#"// Transport is everything needed to reach another service. Whoever deploys
// implements it: HTTP, gRPC, an SDK. The framework picks no transport.
type Transport interface {
	Call(ctx context.Context, target, method string, body any, headers map[string]string) (json.RawMessage, error)
}

// TimedOut and CircuitOpen are the two failures the policy produces itself.
type TimedOut struct{ Who string }

func (e TimedOut) Error() string { return e.Who + ": timed out" }

type CircuitOpen struct{ Who string }

func (e CircuitOpen) Error() string { return e.Who + ": circuit open" }

// Policy is what the manifest declared. The generator writes it; nobody types
// it by hand.
type Policy struct {
	Timeout time.Duration
	Retries int
	Breaker bool
}

// One breaker per target: when the other side goes down, stop hitting it.
// After the cooldown it goes half-open and tries exactly once.
type breaker struct {
	failures  int
	openUntil time.Time
}

const (
	breakerThreshold = 5
	breakerCooldown  = 10 * time.Second
)

var (
	breakersMu sync.Mutex
	breakers   = map[string]*breaker{}
)

func breakerFor(who string) *breaker {
	breakersMu.Lock()
	defer breakersMu.Unlock()
	b, ok := breakers[who]
	if !ok {
		b = &breaker{}
		breakers[who] = b
	}
	return b
}

func (b *breaker) allows(now time.Time) bool {
	breakersMu.Lock()
	defer breakersMu.Unlock()
	return b.openUntil.IsZero() || now.After(b.openUntil)
}

func (b *breaker) succeeded() {
	breakersMu.Lock()
	defer breakersMu.Unlock()
	b.failures, b.openUntil = 0, time.Time{}
}

func (b *breaker) failed(now time.Time) {
	breakersMu.Lock()
	defer breakersMu.Unlock()
	b.failures++
	if b.failures >= breakerThreshold {
		b.openUntil = now.Add(breakerCooldown)
	}
}

// jitter spreads the retries over the whole ceiling. Without it every client
// retries at the same instant and the other side never comes back up.
func jitter(ceiling time.Duration) time.Duration {
	b := make([]byte, 4)
	if _, err := rand.Read(b); err != nil {
		return ceiling / 2
	}
	return time.Duration(uint64(binary.BigEndian.Uint32(b)) % uint64(ceiling))
}

// Headers of an outgoing call: the trace stays the same one.
func Headers(e Envelope, idempotent bool) map[string]string {
	h := map[string]string{
		"traceparent":      e.Traceparent,
		"x-correlation-id": e.CorrelationID,
		"x-causation-id":   e.ID,
	}
	// retrying without a key would duplicate the effect on the other side
	if idempotent {
		h["idempotency-key"] = e.ID
	}
	return h
}

// call applies the declared policy. Retries are only emitted for idempotent
// methods: axon verify blocks the rest.
//
// retriable comes from the CALLEE's declared errors, and it is what makes
// [methods.*] errors more than documentation: a failure the other side
// declared as not retriable is not retried at all. Retrying a declined card
// ends in the same answer and spends the caller's time budget on the way, and
// that budget is what a saga counts on to compensate in time. An error nobody
// declared keeps the old behaviour -retried- because an unknown failure could
// be the network.
func call[T any](
	ctx context.Context,
	t Transport,
	who, target, method string,
	body any,
	h map[string]string,
	pol Policy,
	retriable func(code string) bool,
) (T, error) {
	var zero T
	var b *breaker
	if pol.Breaker {
		b = breakerFor(who)
		if !b.allows(time.Now()) {
			return zero, CircuitOpen{Who: who}
		}
	}
	var last error
	for n := 0; n <= pol.Retries; n++ {
		attempt, cancel := context.WithTimeout(ctx, pol.Timeout)
		raw, err := t.Call(attempt, target, method, body, h)
		cancel()
		if err == nil {
			var out T
			if err = json.Unmarshal(raw, &out); err == nil {
				if b != nil {
					b.succeeded()
				}
				return out, nil
			}
		}
		if errors.Is(err, context.DeadlineExceeded) {
			err = TimedOut{Who: who}
		}
		last = err
		if b != nil {
			b.failed(time.Now())
		}
		// A declared failure that cannot end differently: retrying is spending
		// the budget to get the same answer.
		var p *Problem
		if errors.As(err, &p) && !retriable(p.Code) {
			return zero, err
		}
		if n == pol.Retries {
			break
		}
		ceiling := time.Duration(1<<n) * time.Second
		if ceiling > 10*time.Second {
			ceiling = 10 * time.Second
		}
		select {
		case <-time.After(jitter(ceiling)):
		case <-ctx.Done():
			return zero, ctx.Err()
		}
	}
	return zero, last
}

"#;

/// A typed client per declared dependency, on the service itself.
///
/// The numbers are not a suggestion: the timeout, the retries and the breaker
/// come from `[[depends]]` and land literally in the code, so the policy that
/// ships is the policy that was declared.
fn clients(m: &Manifest, calls: &[crate::contract::Call]) -> String {
    if calls.is_empty() {
        return String::new();
    }
    let t = format!("{}Service", exported(&m.service));
    let mut o = String::from(RESILIENCE_GO);
    for c in calls {
        let (tgt, met) = (&c.service, &c.method);
        let name = format!("{}{}", exported(tgt), exported(met));
        let out = format!("{name}Out");
        let mut doc = vec![format!(
            "{tgt}.{met} · timeout {}ms · {} retries · breaker {}",
            c.timeout_ms, c.retries, c.breaker
        )];
        if !c.declares.is_empty() {
            doc.push(String::new());
            doc.push(format!(
                "\tdeclares: {}",
                c.declares
                    .iter()
                    .map(|(code, status, retriable)| format!(
                        "{code} ({status}{})",
                        if *retriable { ", retriable" } else { "" }
                    ))
                    .collect::<Vec<_>>()
                    .join(" · ")
            ));
        }
        // Go's own spelling of a deprecation, which is what a linter and an
        // editor read: a last paragraph that opens with the word.
        if let Some((sunset, successor)) = &c.retiring {
            doc.push(String::new());
            doc.push(format!(
                "Deprecated: {tgt} announced it{}{}.",
                sunset
                    .as_deref()
                    .map(|s| format!("; it sunsets on {s}"))
                    .unwrap_or_default(),
                successor
                    .as_deref()
                    .map(|s| format!("; use {s}"))
                    .unwrap_or_default(),
            ));
        }
        // An error nobody declared is retried: it could be the network.
        let decide = if c.declares.is_empty() {
            "func(string) bool { return true }".to_string()
        } else if c.retriable.is_empty() {
            "func(string) bool { return false }".to_string()
        } else {
            format!(
                "func(code string) bool {{\n\t\t\treturn {}\n\t\t}}",
                c.retriable
                    .iter()
                    .map(|code| format!("code == {code:?}"))
                    .collect::<Vec<_>>()
                    .join(" || ")
            )
        };
        // `on_partition = "degrade"` makes the degraded path a required
        // argument: the client cannot be called without saying what gets
        // served while the other side is unreachable.
        let (param, tail) = if c.degrades {
            (
                format!(", fallback func() ({out}, error)"),
                "\tif err != nil {\n\
                 \t\t// declared `degrade`: serving something stale beats serving nothing\n\
                 \t\treturn fallback()\n\t}\n\treturn r, nil\n"
                    .to_string(),
            )
        } else {
            (String::new(), "\treturn r, err\n".to_string())
        };
        o.push_str(&format!(
            "{}func (s *{t}) {name}(ctx context.Context, in {name}In, cause Envelope{param}) \
             ({out}, error) {{\n\
             \tr, err := call[{out}](\n\
             \t\tctx,\n\t\ts.transport,\n\t\t{who:?},\n\t\t{tgt:?},\n\t\t{met:?},\n\t\tin,\n\
             \t\tHeaders(cause, {idem}),\n\
             \t\tPolicy{{Timeout: {ms} * time.Millisecond, Retries: {r}, Breaker: {b}}},\n\
             \t\t{decide},\n\t)\n{tail}}}\n\n",
            comment(&doc.join("\n")),
            who = format!("{tgt}.{met}"),
            idem = c.idempotent,
            ms = c.timeout_ms,
            r = c.retries,
            b = c.breaker,
        ));
    }
    o
}

/// In Go the business logic implements an interface; it does not inherit.
fn handlers(m: &Manifest) -> String {
    let s = exported(&m.service);
    let mut o =
        format!("// {s}Handlers is what the person writes.\ntype {s}Handlers interface {{\n");
    for (ev, spec) in &m.consumes {
        o.push_str(&format!(
            "\t// consume {ev}\n\t{}(ctx context.Context, e Envelope, data {}) error\n",
            exported(&spec.handler),
            exported(ev)
        ));
    }
    for name in m.methods.keys() {
        o.push_str(&format!(
            "\t{n}(ctx context.Context, in {n}In, e Envelope) ({n}Out, error)\n",
            n = exported(name)
        ));
    }
    o.push_str("}\n\n");
    o
}

fn service(m: &Manifest) -> String {
    let s = exported(&m.service);
    let t = format!("{s}Service");
    let (sink, mut fields, mut args, mut assign) = match m.patterns.outbox {
        true => (
            "s.outbox.Stage",
            vec![
                vec!["bus".into(), "Bus".into()],
                vec!["inbox".into(), "Inbox".into()],
                vec!["outbox".into(), "Outbox".into()],
            ],
            "bus Bus, inbox Inbox, outbox Outbox".to_string(),
            "bus: bus, inbox: inbox, outbox: outbox".to_string(),
        ),
        false => (
            "s.bus.Publish",
            vec![
                vec!["bus".into(), "Bus".into()],
                vec!["inbox".into(), "Inbox".into()],
            ],
            "bus Bus, inbox Inbox".to_string(),
            "bus: bus, inbox: inbox".to_string(),
        ),
    };
    fields.push(vec!["h".into(), format!("{s}Handlers")]);
    args.push_str(&format!(", h {s}Handlers"));
    assign.push_str(", h: h");
    // Only what the service USES: asking for a transport from one that calls
    // nobody forces making up a fake one, and a fake in the constructor is a
    // dependency nobody reviews.
    if !m.depends.is_empty() {
        fields.push(vec!["transport".into(), "Transport".into()]);
        args.push_str(", transport Transport");
        assign.push_str(", transport: transport");
    }
    let mut o = format!("type {t} struct {{\n{}}}\n\n", aligned(&fields));
    o.push_str(&format!(
        "func New{t}({args}) *{t} {{\n\treturn &{t}{{{assign}}}\n}}\n\n"
    ));
    for ev in m.emits.keys() {
        o.push_str(&format!(
            "func (s *{t}) Emit{e}(ctx context.Context, data {e}, cause *Envelope) error {{\n\
             \te, err := NewEnvelope({ev:?}, {svc:?}, data, cause)\n\
             \tif err != nil {{\n\t\treturn err\n\t}}\n\
             \treturn {sink}(ctx, e)\n}}\n\n",
            e = exported(ev),
            svc = m.service
        ));
    }
    if !m.consumes.is_empty() {
        o.push_str(&format!(
            "// Dispatch is the single entry point: it deduplicates by id and\n\
             // routes by type.\n\
             func (s *{t}) Dispatch(ctx context.Context, e Envelope) error {{\n\
             \treturn s.inbox.Once(ctx, e.ID, func(ctx context.Context) error {{\n\
             \t\tswitch e.Type {{\n"
        ));
        for (ev, spec) in &m.consumes {
            o.push_str(&format!(
                "\t\tcase {ev:?}:\n\
                 \t\t\tvar data {e}\n\
                 \t\t\tif err := json.Unmarshal(e.Data, &data); err != nil {{\n\
                 \t\t\t\treturn err\n\t\t\t}}\n\
                 \t\t\treturn s.h.{h}(ctx, e, data)\n",
                e = exported(ev),
                h = exported(&spec.handler)
            ));
        }
        o.push_str(&format!(
            "\t\tdefault:\n\t\t\treturn fmt.Errorf({:?}, e.Type)\n\t\t}}\n\t}})\n}}\n\n",
            format!("{}: type not declared in the manifest: %s", m.service)
        ));
    }
    o
}

fn machines(m: &Manifest) -> String {
    let mut o = String::new();
    for (name, mac) in &m.machine {
        let n = exported(name);
        o.push_str(&format!("type {n}State string\ntype {n}Action string\n\n"));
        o.push_str(&format!(
            "var {n}Transitions = map[{n}Action]struct {{\n\tFrom []{n}State\n\tTo   {n}State\n\tOn   string\n}}{{\n"
        ));
        let rows: Vec<Vec<String>> = mac
            .transitions
            .iter()
            .map(|(act, t)| {
                let from: Vec<String> = t.from.iter().map(|f| format!("{f:?}")).collect();
                vec![
                    format!("{act:?}:"),
                    format!(
                        "{{From: []{n}State{{{}}}, To: {:?}, On: {:?}}},",
                        from.join(", "),
                        t.to,
                        t.on
                    ),
                ]
            })
            .collect();
        o.push_str(&aligned(&rows));
        o.push_str("}\n\n");
        o.push_str(&format!(
            "// {n}Next fails if the transition is not declared in the manifest.\n\
             func {n}Next(state {n}State, action {n}Action) ({n}State, error) {{\n\
             \tt, ok := {n}Transitions[action]\n\
             \tif !ok {{\n\t\treturn \"\", fmt.Errorf({:?}, action)\n\t}}\n\
             \tfor _, f := range t.From {{\n\t\tif f == state {{\n\t\t\treturn t.To, nil\n\t\t}}\n\t}}\n\
             \treturn \"\", fmt.Errorf({:?}, action, state)\n}}\n\n\
             func {n}Can(state {n}State, action {n}Action) bool {{\n\
             \t_, err := {n}Next(state, action)\n\treturn err == nil\n}}\n\n",
            format!("{name}: unknown action: %s"),
            format!("{name}: %s is not legal from %s")
        ));
    }
    o
}

pub fn build(m: &Manifest, all: &[Manifest]) -> Result<String, String> {
    let pkg = m.service.replace(['-', '_'], "");
    let c = crate::contract::of(m, all)?;
    let body = format!(
        "{}{}{}{}{}{}{}{}",
        types(&c),
        problem(m),
        failures(m),
        scopes(m),
        handlers(m),
        service(m),
        clients(m, &c.calls),
        machines(m)
    );
    // gofmt leaves exactly one newline at the end of a file
    Ok(format!("{}{}\n", header(&pkg, &body), body.trim_end()))
}
