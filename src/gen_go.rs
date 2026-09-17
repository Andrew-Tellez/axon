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

/// A field's name as a parameter takes it: the exported spelling with its
/// first letter down, so `tenant_id` is `tenantID` and not `tenantId`.
fn unexported(s: &str) -> String {
    let f = field(s);
    let mut c = f.chars();
    match c.next() {
        Some(first) => first.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
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

/// A JSON value as a Go literal.
///
/// Only object flags need it, and they need it for the same reason the other
/// three do not: the declared default has to be IN the code, so a provider
/// that is down or has never heard of the flag still answers what the manifest
/// said was safe.
fn go_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "nil".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("{s:?}"),
        serde_json::Value::Array(a) => format!(
            "[]any{{{}}}",
            a.iter().map(go_literal).collect::<Vec<_>>().join(", ")
        ),
        serde_json::Value::Object(o) => format!(
            "map[string]any{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{k:?}: {}", go_literal(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The same OpenFeature shape as the TypeScript one, in the only form Go has
/// for it: an interface cannot carry a type parameter, so the provider answers
/// `any` and one generic function checks that what came back is what was
/// declared.
const FLAGS_GO: &str = r#"// Flags is a provider with OpenFeature's shape: Evaluate takes the name, the
// default value and the context the decision is pinned by. axon ships no flag
// SDK and invents no protocol, same as it ships none for traces. What it adds
// is that the name, the safe value and the field it is pinned by come out of
// the manifest and not out of a loose string in the code.
type Flags interface {
	Evaluate(ctx context.Context, name string, fallback any, context map[string]string) (any, error)
}

// flagValue asks the provider and checks that what came back is the type that
// was declared. A provider answering a string to a boolean flag is a
// misconfiguration, and the declared safe value is a better answer to it than
// a panic in the path of a request.
func flagValue[T any](ctx context.Context, f Flags, name string, fallback T, context map[string]string) (T, error) {
	v, err := f.Evaluate(ctx, name, fallback, context)
	if err != nil {
		return fallback, err
	}
	t, ok := v.(T)
	if !ok {
		return fallback, fmt.Errorf("flag %s: the provider answered %T, and %T was declared", name, v, fallback)
	}
	return t, nil
}

"#;

/// Typed accessors for the declared flags.
fn flags(m: &Manifest) -> String {
    if m.flags.is_empty() {
        return String::new();
    }
    let mut o = String::from(FLAGS_GO);
    let mut names = Vec::new();
    for (name, f) in &m.flags {
        names.push(format!("{name:?}"));
        let variants = f.all_variants();
        let value = variants
            .get(&f.default_variant())
            .map(go_literal)
            .unwrap_or_else(|| "false".into());
        let kind = match f.kind() {
            "boolean" => "bool",
            "string" => "string",
            "number" => "float64",
            _ => "map[string]any",
        };
        let mut doc = vec![format!(
            "{} is the `{name}` flag: OpenFeature {}.",
            format_args!("Flag{}", exported(name)),
            f.kind()
        )];
        if variants.len() > 2 || !f.variants.is_empty() {
            doc.push(format!(
                "Variants: {}.",
                variants
                    .iter()
                    .map(|(k, v)| format!("{k} = {}", go_literal(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(c) = &f.sticky_by {
            doc.push(format!(
                "Pinned by {c}: the same entity always takes the same path."
            ));
        }
        // The context is the field the decision is pinned by, under its own
        // name and under OpenFeature's: a provider reads one or the other.
        let (param, context) = match &f.sticky_by {
            Some(c) => {
                let arg = unexported(c);
                (
                    format!(", {arg} string"),
                    format!("map[string]string{{\"targetingKey\": {arg}, {c:?}: {arg}}}"),
                )
            }
            None => (String::new(), "map[string]string{}".to_string()),
        };
        // A number needs saying which one: an untyped `3` makes the generic
        // infer `int`, and the provider answers the `float64` that JSON has.
        // The literal of the other three already carries its type.
        let safe = match kind {
            "float64" => format!("float64({value})"),
            _ => value,
        };
        o.push_str(&format!(
            "{}func Flag{n}(ctx context.Context, flags Flags{param}) ({kind}, error) {{\n\
             \treturn flagValue(ctx, flags, {name:?}, {safe}, {context})\n}}\n\n",
            comment(&doc.join("\n\n")),
            n = exported(name),
        ));
    }
    o.push_str(&format!(
        "// DeclaredFlags are the flags the manifest declares. One that is not\n\
         // here does not exist.\n\
         var DeclaredFlags = []string{{{}}}\n\n",
        names.join(", ")
    ));
    o
}

/// What each subscription asks of the broker, so the consumer is wired from
/// here and not from memory. The same numbers `axon infra` prints in the
/// commands that create them.
fn subscriptions(m: &Manifest) -> String {
    if m.consumes.is_empty() {
        return String::new();
    }
    let doc = comment(
        "Subscription is what a consumer asks of the broker.\n\nAckWaitMS is how long the \
         handler has before the event comes back. Shorter than it really takes and it runs \
         twice, with nothing saying so: the dedup by envelope id is what makes that survivable, \
         not harmless.",
    );
    let mut o = format!(
        "{doc}type Subscription struct {{\n\tGroup      string\n\tMaxDeliver int\n\t\
         OrderedBy  string\n\tAckWaitMS  int\n}}\n\n\
         // Subscriptions is what the manifest declared, by event.\n\
         var Subscriptions = map[string]Subscription{{\n"
    );
    for (ev, c) in &m.consumes {
        o.push_str(&format!(
            "\t{ev:?}: {{Group: {:?}, MaxDeliver: {}, OrderedBy: {:?}, AckWaitMS: {}}},\n",
            c.group(&m.service),
            c.max_deliver(),
            c.ordered_by.clone().unwrap_or_default(),
            c.ack_wait_ms.unwrap_or(0),
        ));
    }
    o.push_str("}\n\n");
    o
}

/// The task queue in Go: the same table, the same claim, the same ceiling.
fn tasks_go(m: &Manifest) -> String {
    if m.tasks.is_empty() {
        return String::new();
    }
    let mut o = comment(
        "TaskQueue is the queue, which is a table in this service's own database. \
         Enqueuing inside the transaction that changed the row is one more write —not a second \
         commit that can be lost.\n\nClaim is the one that cannot be written casually: two \
         workers taking the same row run the task twice. It has to be one statement with FOR \
         UPDATE SKIP LOCKED, which is what makes two workers take different rows, and a \
         staleBefore on the rows left running, which is what brings back what a worker that \
         died was holding.",
    );
    o.push_str(
        "type TaskRow struct {\n\tID       string\n\tAttempts int\n\tData     Envelope\n}\n\n\
         type TaskQueue interface {\n\t\
           Enqueue(ctx context.Context, id, name string, runAt time.Time, data Envelope) error\n\t\
           Claim(ctx context.Context, name string, due, staleBefore time.Time, limit int) ([]TaskRow, error)\n\t\
           Done(ctx context.Context, id string) error\n\t\
           Retry(ctx context.Context, id string, runAt time.Time) error\n\t\
           Dead(ctx context.Context, id string, reason error) error\n\t\
           Read(ctx context.Context, id string) (status string, attempts int, err error)\n\
         }\n\n",
    );
    o.push_str(&comment(
        "TaskReport is what one pass did. Pending says the limit filled up and there is more \
         waiting, which is the difference between a worker that keeps up and one that does not.",
    ));
    o.push_str(
        "type TaskReport struct {\n\tClaimed int\n\tDone    int\n\tRetried int\n\tDead    int\n\t\
         Pending bool\n}\n\n",
    );

    for (name, t) in &m.tasks {
        let p = exported(name);
        o.push_str(&struct_of(
            &format!("{p}Task"),
            &t.input.iter().collect::<Vec<_>>(),
            &format!("{p}Task is what {name} is enqueued with."),
        ));
        // Only when there is one to apply: `if delay == 0 { delay = 0 }` is a
        // knob that reads as a decision and is not one.
        let default_delay = match t.delay_ms.unwrap_or(0) {
            0 => String::new(),
            ms => format!("if delay == 0 {{\n\t\tdelay = {ms} * time.Millisecond\n\t}}\n\t"),
        };
        o.push_str(&comment(&format!(
            "Enqueue{p} enqueues {name} and returns the id, which is the receipt: it is the \
             only thing the caller keeps, and TaskQueue.Read is where it is looked up. Call it \
             inside the transaction that made the work necessary.",
        )));
        o.push_str(&format!(
            "func Enqueue{p}(ctx context.Context, q TaskQueue, in {p}Task, cause Envelope, \
             delay time.Duration) (string, error) {{\n\t\
               {default_delay}\
               id := uuid4()\n\t\
               // The envelope travels with it: the trace of whoever enqueued it is what\n\t\
               // connects the work to the request that caused it.\n\t\
               e, err := NewEnvelope(\"task.{name}\", {svc:?}, in, &cause)\n\t\
               if err != nil {{\n\t\treturn \"\", err\n\t}}\n\t\
               return id, q.Enqueue(ctx, id, {name:?}, time.Now().Add(delay), e)\n\
             }}\n\n",
            svc = m.service,
        ));

        let limit = t.concurrency.unwrap_or(1);
        let stale = match t.timeout_ms {
            Some(ms) => format!("{ms} * time.Millisecond"),
            // Nothing to measure a hung run against, so nothing is reclaimed.
            None => "time.Duration(1<<62)".into(),
        };
        o.push_str(&comment(&format!(
            "Run{p} is one pass of the {name} queue: it claims what is due and runs it, up to \
             {limit} at a time —the declared concurrency— because a queue that drains as fast \
             as the database allows is how a batch takes down the database the rows are read \
             from.\n\nA run that fails goes back to waiting with exponential backoff until \
             attempt {}; after that it is left dead where somebody can see it.",
            t.max_deliver()
        )));
        o.push_str(&format!(
            "func Run{p}(ctx context.Context, q TaskQueue, h func(context.Context, {p}Task, \
             Envelope) error, limit int) (TaskReport, error) {{\n\t\
               if limit <= 0 {{\n\t\tlimit = {limit}\n\t}}\n\t\
               now := time.Now()\n\t\
               rows, err := q.Claim(ctx, {name:?}, now, now.Add(-({stale})), limit)\n\t\
               if err != nil {{\n\t\treturn TaskReport{{}}, err\n\t}}\n\t\
               r := TaskReport{{Claimed: len(rows), Pending: len(rows) >= limit}}\n\t\
               var mu sync.Mutex\n\t\
               var wg sync.WaitGroup\n\t\
               for _, row := range rows {{\n\t\t\
                 wg.Add(1)\n\t\t\
                 go func(row TaskRow) {{\n\t\t\t\
                   defer wg.Done()\n\t\t\t\
                   var in {p}Task\n\t\t\t\
                   runErr := json.Unmarshal(row.Data.Data, &in)\n\t\t\t\
                   if runErr == nil {{\n\t\t\t\t\
                     runErr = h(ctx, in, row.Data)\n\t\t\t}}\n\t\t\t\
                   mu.Lock()\n\t\t\t\
                   defer mu.Unlock()\n\t\t\t\
                   if runErr == nil {{\n\t\t\t\t\
                     if err := q.Done(ctx, row.ID); err == nil {{\n\t\t\t\t\t\
                       r.Done++\n\t\t\t\t}}\n\t\t\t\t\
                     return\n\t\t\t}}\n\t\t\t\
                   if row.Attempts >= {attempts} {{\n\t\t\t\t\
                     if err := q.Dead(ctx, row.ID, runErr); err == nil {{\n\t\t\t\t\t\
                       r.Dead++\n\t\t\t\t}}\n\t\t\t\t\
                     return\n\t\t\t}}\n\t\t\t\
                   // Exponential from a second: a task that fails because what it calls\n\t\t\t\
                   // is down should not hammer it while it comes back.\n\t\t\t\
                   back := time.Duration(1<<uint(row.Attempts)) * time.Second\n\t\t\t\
                   if err := q.Retry(ctx, row.ID, time.Now().Add(back)); err == nil {{\n\t\t\t\t\
                     r.Retried++\n\t\t\t}}\n\t\t\
                 }}(row)\n\t}}\n\t\
               wg.Wait()\n\t\
               return r, nil\n\
             }}\n\n",
            attempts = t.max_deliver()
        ));
    }

    o.push_str(&comment(
        "Tasks is what the manifest declares, with the route axon infra points a scheduler at. \
         Startup has to serve each one by calling its Run: a scheduler aimed at a 404 applies \
         with no error and drains nothing.",
    ));
    o.push_str("var Tasks = map[string]string{\n");
    for name in m.tasks.keys() {
        o.push_str(&format!(
            "\t{name:?}: \"POST {}\",\n",
            Task::run_route(name)
        ));
    }
    o.push_str("}\n\n");
    o
}

/// The streams in Go: the same wire format and the same obligation.
fn sse_go(m: &Manifest) -> String {
    if m.sse.is_empty() {
        return String::new();
    }
    let mut o = comment(
        "SSEFrame is one frame, as the wire wants it.\n\nThe payload goes through json.Marshal, \
         which never emits a raw newline —it escapes them inside strings— so it is always ONE \
         data: line. That is the reason the frame is built from JSON and not from whatever the \
         caller has at hand: a raw newline in the payload ends the frame early and the client \
         reads a truncated event, with nothing saying so.\n\nThe id travels so the client can \
         send it back as Last-Event-ID when it reconnects. What to do with it on the way back is \
         yours: it is only answerable if there is something to replay from.",
    );
    o.push_str(
        "func SSEFrame(id, event string, data any, retryMS int) []byte {\n\t\
           body, err := json.Marshal(data)\n\t\
           if err != nil {\n\t\tbody = []byte(`null`)\n\t}\n\t\
           var b strings.Builder\n\t\
           if retryMS > 0 {\n\t\t\
             fmt.Fprintf(&b, \"retry: %d\\n\", retryMS)\n\t}\n\t\
           fmt.Fprintf(&b, \"id: %s\\nevent: %s\\ndata: %s\\n\\n\", id, event, body)\n\t\
           return []byte(b.String())\n}\n\n",
    );
    o.push_str(&comment(
        "SSEHeartbeat is a comment line. It is not for the client — it is what keeps a proxy \
         from closing a connection it believes idle, and a client reconnecting in a loop is the \
         symptom of not sending it.",
    ));
    o.push_str("var SSEHeartbeat = []byte(\": keep-alive\\n\\n\")\n\n");
    o.push_str(&comment(
        "SSEStreams is what the manifest declares. Startup must serve each Path by holding the \
         response open and writing frames: a route that answers and closes is a client that \
         reconnects forever.",
    ));
    o.push_str(
        "type SSEStream struct {\n\t\
           Path        string\n\t\
           Auth        string\n\t\
           Scopes      []string\n\t\
           Events      []string\n\t\
           HeartbeatMS int\n\t\
           RetryMS     int\n}\n\n\
         var SSEStreams = map[string]SSEStream{\n",
    );
    for (name, st) in &m.sse {
        let list = |v: &[String]| {
            v.iter()
                .map(|x| format!("{x:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        o.push_str(&format!(
            "\t{name:?}: {{Path: {:?}, Auth: {:?}, Scopes: []string{{{}}}, Events: []string{{{}}}, \
             HeartbeatMS: {}, RetryMS: {}}},\n",
            st.path.clone().unwrap_or_default(),
            st.auth.clone().unwrap_or_default(),
            list(&st.scopes),
            list(&st.events),
            st.heartbeat_ms(),
            st.retry_ms.unwrap_or(0),
        ));
    }
    o.push_str("}\n\n");
    o
}

/// The socket in Go: the policy, the type table, and the dispatch that routes
/// a frame to the method it already implements.
fn ws_go(m: &Manifest) -> String {
    if !m.ws.declared() {
        return String::new();
    }
    let t = format!("{}Service", exported(&m.service));
    let list = |v: &[String]| {
        v.iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut o = comment(
        "WSRoute is where the handshake lands. It is a route of the edge like any other, and \
         axon infra renders it as one.",
    );
    o.push_str(&format!(
        "const WSRoute = {:?}\n\n",
        m.ws.path.clone().unwrap_or_default()
    ));
    o.push_str(&comment(
        "WSPolicy is what the edge cannot apply for you. RateLimit is per CONNECTION and per \
         minute: after the upgrade the edge has seen one request, and every message after it is \
         invisible to it.",
    ));
    o.push_str(&format!(
        "var WSPolicy = struct {{\n\t\
           Auth            string\n\t\
           Scopes          []string\n\t\
           HeartbeatMS     int\n\t\
           RateLimit       int\n\t\
           MaxMessageBytes int\n\t\
           Origins         []string\n\
         }}{{\n\t\
           Auth:            {:?},\n\t\
           Scopes:          []string{{{}}},\n\t\
           HeartbeatMS:     {},\n\t\
           RateLimit:       {},\n\t\
           MaxMessageBytes: {},\n\t\
           Origins:         []string{{{}}},\n\
         }}\n\n",
        m.ws.auth.clone().unwrap_or_default(),
        list(&m.ws.scopes),
        m.ws.heartbeat_ms.unwrap_or(0),
        m.ws.rate_limit.unwrap_or(0),
        m.ws.max_message_bytes(),
        list(&m.ws.origins),
    ));
    o.push_str(&comment(
        "WSMessages is the declared types and the method each one lands on. Startup can refuse \
         what it does not recognise instead of discovering it on the first frame.",
    ));
    o.push_str("var WSMessages = map[string]string{\n");
    for (name, me) in &m.methods {
        if let Some(ty) = me.ws.as_deref() {
            o.push_str(&format!("\t{ty:?}: {name:?},\n"));
        }
    }
    o.push_str("}\n\n");
    o.push_str(&comment(
        "WSOriginAllowed says whether a page from this origin may open the socket. CORS does NOT \
         apply to a WebSocket —the handshake is not a cross-origin request the browser blocks— so \
         Origin is the only thing that says where it came from. With no declared list this \
         answers true, and axon verify says so.",
    ));
    o.push_str(
        "func WSOriginAllowed(origin string) bool {\n\t\
           if len(WSPolicy.Origins) == 0 {\n\t\treturn true\n\t}\n\t\
           for _, o := range WSPolicy.Origins {\n\t\t\
             if o == origin {\n\t\t\treturn true\n\t\t}\n\t}\n\t\
           return false\n}\n\n",
    );
    o.push_str(&comment(
        "WSFrame is what travels in both directions. ID correlates the answer with the request: \
         without it a client with two in flight cannot tell which answer is whose, and over a \
         socket there is no request/response pairing to fall back on.",
    ));
    o.push_str(
        "type WSFrame struct {\n\t\
           ID    string          `json:\"id\"`\n\t\
           Type  string          `json:\"type,omitempty\"`\n\t\
           Data  json.RawMessage `json:\"data,omitempty\"`\n\t\
           Error string          `json:\"error,omitempty\"`\n\
         }\n\n",
    );
    o.push_str(&comment(
        "DispatchWS parses a frame, routes it by type and answers correlated by the same id. The \
         implementation is the method's own: one body, two transports. A second one is what \
         would let HTTP and the socket answer differently.",
    ));
    let mut arms = String::new();
    for (name, me) in &m.methods {
        let Some(ty) = me.ws.as_deref() else { continue };
        let n = exported(name);
        arms.push_str(&format!(
            "\tcase {ty:?}:\n\t\t\
               var in {n}In\n\t\t\
               if err := json.Unmarshal(msg.Data, &in); err != nil {{\n\t\t\t\
                 return fail(msg.ID, \"malformed\"), nil\n\t\t}}\n\t\t\
               out, err := s.h.{n}(ctx, in, e)\n\t\t\
               if err != nil {{\n\t\t\treturn nil, err\n\t\t}}\n\t\t\
               return json.Marshal(WSFrame{{ID: msg.ID, Data: mustJSON(out)}})\n"
        ));
    }
    o.push_str(&format!(
        "func (s *{t}) DispatchWS(ctx context.Context, raw []byte, e Envelope) ([]byte, error) \
         {{\n\t\
           // Before parsing, not after: the ceiling exists so one message is not an\n\t\
           // allocation of whatever the other side felt like sending.\n\t\
           if len(raw) > WSPolicy.MaxMessageBytes {{\n\t\t\
             return fail(\"\", \"message_too_large\"), nil\n\t}}\n\t\
           var msg WSFrame\n\t\
           if err := json.Unmarshal(raw, &msg); err != nil {{\n\t\t\
             return fail(\"\", \"malformed\"), nil\n\t}}\n\t\
           switch msg.Type {{\n\
         {arms}\t}}\n\t\
           // Named, not swallowed: a type the manifest does not declare is a client\n\t\
           // that believes it is sending something real.\n\t\
           return fail(msg.ID, \"type_not_declared\"), nil\n\
         }}\n\n\
         func fail(id, code string) []byte {{\n\t\
           b, _ := json.Marshal(WSFrame{{ID: id, Error: code}})\n\t\
           return b\n\
         }}\n\n\
         func mustJSON(v any) json.RawMessage {{\n\t\
           b, err := json.Marshal(v)\n\t\
           if err != nil {{\n\t\treturn json.RawMessage(`null`)\n\t}}\n\t\
           return b\n\
         }}\n\n"
    ));
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
    for (name, t) in &m.tasks {
        o.push_str(&format!(
            "\t// task {name}\n\t{}(ctx context.Context, in {}Task, e Envelope) error\n",
            exported(&t.handler),
            exported(name)
        ));
    }
    for name in m.sse.keys() {
        o.push_str(&format!(
            "\t// does this event belong on THIS connection? the only part of a stream\n\t\
             // that knows the domain, so it is an obligation and not a default\n\t\
             Stream{}(ctx context.Context, e Envelope, params map[string]string) bool\n",
            exported(name)
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
        "{}{}{}{}{}{}{}{}{}{}{}{}{}",
        types(&c),
        problem(m),
        failures(m),
        scopes(m),
        subscriptions(m),
        tasks_go(m),
        ws_go(m),
        sse_go(m),
        handlers(m),
        service(m),
        clients(m, &c.calls),
        flags(m),
        machines(m)
    );
    // gofmt leaves exactly one newline at the end of a file
    Ok(format!("{}{}\n", header(&pkg, &body), body.trim_end()))
}
