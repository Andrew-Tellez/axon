// The playground: the real compiler, running in the tab.
//
// No server behind it and no prerecorded output. `axon.js` and `axon_bg.wasm`
// are the same `verify` that runs in the terminal, compiled to wasm: what the
// page says is what CI would say.
import init, { verify, graph, contracts } from "./axon.js";

const FILES = {
  "orders.toml": `service = "orders"
owner = "commerce-team"
tier = "1"
version = "1.0.0"

# The side of the CAP theorem, said out loud. Everything else follows from it:
# the isolation level, whether a replica can be read, what happens on a partition.
[cap]
consistency = "eventual"
on_partition = "degrade"
max_staleness_ms = 3000

[infra]
state = "postgres"
migrations = "sql/orders"

# The order is written and the event published in the same transaction.
[patterns]
outbox = true

[emits."order.placed@v1"]
tenantId = "uuid"
orderId = "uuid"
total = "int"

[methods.placeOrder]
http = "POST /v1/orders"
auth = "required"
scopes = ["orders:write"]
idempotent = true
in = { tenantId = "uuid", total = "int" }
out = { orderId = "uuid" }

[analytics]
retention_days = 365

[auth]
issuers = ["https://auth.acme.mx"]
audience = "orders"
verify = "jwks"
jwks_uri = "https://auth.acme.mx/.well-known/jwks.json"
algorithms = ["EdDSA", "ES256"]
revocation = "eventual"
max_token_age_s = 900
subject_claim = "sub"
tenant_claim = "org_id"
scopes_claim = "scope"

# A synchronous call to somebody else: what it costs when it fails is declared
# here, not left to whatever the HTTP client happens to do.
[[depends]]
service = "pay"
method = "charges.create"
timeout_ms = 2000
retries = 2
breaker = true
uses = ["id", "status"]
`,
  "notifier.toml": `service = "notifier"
owner = "growth-team"
tier = "2"
version = "1.0.0"

[cap]
consistency = "eventual"
on_partition = "degrade"
max_staleness_ms = 60000

# What this service READS of the event. The rest stays free to change.
[consumes."order.placed@v1"]
handler = "onOrderPlaced"
uses = ["orderId", "total"]
`,
  "pay.external.toml": `# An external service: axon does not compile it, it freezes its contract here
# so \`verify\` can say whether the generated client and the provider still agree.
service = "pay"
external = true

[methods."charges.create"]
idempotent = true
in = { amount = "int", orderId = "uuid" }
out = { id = "string", status = "string" }
`,
  "sql/orders/001_init.sql": `create table orders (
  id uuid primary key,
  tenant_id uuid not null,
  total int not null
);

-- The outbox \`[patterns] outbox = true\` promises: without this table the event
-- is published outside the transaction, and there is a window where the row
-- exists and nobody received anything.
create table outbox (
  id uuid primary key,
  topic text not null,
  payload jsonb not null,
  published_at timestamptz
);
`,
};

// Each exercise breaks ONE thing, and the rule that fires is the lesson. They
// apply to what is written, not to the original: they stack.
const EXERCISES = [
  {
    label: "Remove the consumer",
    hint: "An event nobody reads is a contract nobody holds up.",
    file: "notifier.toml",
    apply: (t) => t.split("[consumes")[0].trimEnd() + "\n",
  },
  {
    label: "Remove the outbox",
    hint: "The table goes, and the manifest's promise is left on its own.",
    file: "sql/orders/001_init.sql",
    apply: (t) => t.split("-- The outbox")[0].trimEnd() + "\n",
  },
  {
    label: "Promise strong consistency",
    hint: "The guarantee says one thing and the topology says another.",
    file: "orders.toml",
    apply: (t) => t.replace('consistency = "eventual"', 'consistency = "strong"'),
  },
  {
    label: "Read a field nobody declared",
    hint: "The handler reads `customerEmail` and the emitter does not send it.",
    file: "notifier.toml",
    apply: (t) => t.replace('uses = ["orderId", "total"]', 'uses = ["orderId", "customerEmail"]'),
  },
];

const original = { ...FILES };
let active = "orders.toml";

const $ = (id) => document.getElementById(id);

// Which answer is on screen. Only that one is computed: the compiler is fast,
// but generating 450 lines of TypeScript on every keystroke to show a diagram
// is work nobody asked for.
let view = "report";

function show(next) {
  view = next;
  for (const b of $("pg-views").children) b.classList.toggle("pg-on", b.dataset.v === view);
  for (const name of ["report", "topology", "contracts"]) {
    $(`pg-pane-${name}`).hidden = name !== view;
  }
  run();
}

function run() {
  if (view === "topology") return void draw();
  if (view === "contracts") {
    $("pg-ts").textContent = contracts(JSON.stringify(Object.entries(FILES)), active);
    return;
  }
  const out = $("pg-out");
  let report;
  try {
    report = JSON.parse(verify(JSON.stringify(Object.entries(FILES))));
  } catch (e) {
    out.innerHTML = `<li class="pg-e">the compiler fell over: ${e}</li>`;
    return;
  }
  const { errors, warnings } = report;
  $("pg-count").textContent =
    errors.length === 0 && warnings.length === 0
      ? "no findings"
      : `${errors.length} error(s) · ${warnings.length} warning(s)`;
  $("pg-count").className = errors.length ? "pg-bad" : warnings.length ? "pg-warn" : "pg-ok";

  if (!errors.length && !warnings.length) {
    out.innerHTML =
      '<li class="pg-ok">Everything agrees. Break something with the buttons above, or type in the editor.</li>';
    return;
  }
  const item = (kind, text) =>
    `<li class="pg-${kind}"><b>${kind === "e" ? "error" : "warn"}</b> ${escape_(text)}</li>`;
  out.innerHTML =
    errors.map((e) => item("e", e)).join("") + warnings.map((w) => item("w", w)).join("");
}

// The same `axon graph` as the terminal: the event topology as mermaid, drawn
// by the mermaid the book already loads for every other page.
let drawn = "";
let seq = 0;
async function draw() {
  const code = graph(JSON.stringify(Object.entries(FILES)));
  // redrawing the identical thing flickers for nothing, and this runs on
  // every keystroke
  if (!code || code === drawn || !window.mermaid) return;
  drawn = code;
  try {
    const { svg } = await window.mermaid.render(`pg-svg-${seq++}`, code);
    $("pg-graph").innerHTML = svg;
  } catch {
    // a half-typed manifest makes a graph mermaid will not take: the last good
    // one stays, which is more use than a hole
    drawn = "";
  }
}

function escape_(s) {
  const d = document.createElement("div");
  d.textContent = s;
  // the findings carry `this` in backticks: it reads better as code
  return d.innerHTML.replace(/`([^`]+)`/g, "<code>$1</code>");
}

function tabs() {
  $("pg-tabs").innerHTML = Object.keys(FILES)
    .map(
      (name) =>
        `<button data-f="${name}" class="${name === active ? "pg-on" : ""}">${name}</button>`,
    )
    .join("");
  for (const b of $("pg-tabs").children) {
    b.onclick = () => {
      FILES[active] = $("pg-src").value;
      active = b.dataset.f;
      $("pg-src").value = FILES[active];
      tabs();
      // the contracts are the ones of the file being edited
      if (view === "contracts") run();
    };
  }
}

let timer;
function wire() {
  $("pg-src").value = FILES[active];
  $("pg-src").oninput = () => {
    FILES[active] = $("pg-src").value;
    // the compiler takes ~15 ms; the delay is so it does not run per keystroke
    clearTimeout(timer);
    timer = setTimeout(run, 150);
  };
  $("pg-ex").innerHTML = EXERCISES.map(
    (e, i) => `<button data-i="${i}" title="${e.hint}">${e.label}</button>`,
  ).join("");
  for (const b of $("pg-ex").children) {
    b.onclick = () => {
      const e = EXERCISES[b.dataset.i];
      FILES[e.file] = e.apply(FILES[e.file]);
      active = e.file;
      $("pg-src").value = FILES[active];
      $("pg-hint").textContent = e.hint;
      tabs();
      run();
    };
  }
  for (const b of $("pg-views").children) b.onclick = () => show(b.dataset.v);
  $("pg-reset").onclick = () => {
    Object.assign(FILES, original);
    $("pg-src").value = FILES[active];
    $("pg-hint").textContent = "";
    run();
  };
  tabs();
}

init().then(() => {
  wire();
  run();
  // the book loads mermaid as a classic script, so it is there before this
  // module runs — but if it ever is not, the diagram would stay blank until
  // the first keystroke, and that reads as a broken page
  if (!window.mermaid) window.addEventListener("load", draw, { once: true });
});
