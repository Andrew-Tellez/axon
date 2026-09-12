# Playground

The compiler runs **in this tab**. There is no server behind it and no prerecorded
output: `axon verify` is compiled to WebAssembly, so what this page says is what CI would
say about the same files.

Two services and one migration. They are clean —zero errors, zero warnings— and the way
to learn what axon checks is to **break them**. Each button breaks exactly one thing; the
rule that fires is the lesson. The editor is yours too.

<div class="pg">
  <div class="pg-bar">
    <span id="pg-ex"></span>
    <button id="pg-reset">back to the original</button>
  </div>
  <p id="pg-hint" class="pg-hint"></p>
  <div class="pg-tabs" id="pg-tabs"></div>
  <textarea id="pg-src" spellcheck="false" rows="24"></textarea>
  <p class="pg-head">the report · <span id="pg-count">compiling…</span></p>
  <ul id="pg-out"><li>loading the compiler…</li></ul>
</div>

<script type="module" src="playground/app.js"></script>

## What is not here

With no filesystem, the web build leaves out two things the CLI does look at: the
`Dockerfile` the compose expects, and any migration you do not hand it as text.
Everything else —the guarantees, the event topology, the contracts, the schema, the
scopes, the retention— is the same code.

And there are no generators: `build`, `infra`, `openapi` and the diagrams live in the
CLI. This page is for understanding **what gets checked**, not for generating a repo.
For that:

```sh
curl -fsSL https://axon.andrewtellez.dev/install.sh | sh
```
