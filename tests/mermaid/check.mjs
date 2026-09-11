// Parses a mermaid diagram with mermaid ITSELF, and says nothing else.
//
// The rule of this suite is that a generator is verified with the real tool of
// its ecosystem, and the five diagrams were the only generators verified
// against their own golden. A golden says the text did not change; it does not
// say the text renders. `axon graph` spent a version dying to parse on every
// line that names an event, because `@` stopped being text in mermaid 11 and
// no assert here was looking at what mermaid thinks.
//
// It also works by hand, which is the point of it being a file and not an
// inline string in the test:
//
//   axon classes . | node tests/mermaid/check.mjs
//   node tests/mermaid/check.mjs a.mmd b.mmd
//
// Run by hand it also proves the newlines survived the trip: a `}` and the
// `class` after it on one line —a paste out of a wrapped terminal— is a parse
// error that points at the wrong line and blames the wrong thing.
import fs from "node:fs";
import { JSDOM } from "jsdom";

// mermaid is a browser library: it reads `document` while it loads, not when
// it is called, so the DOM has to exist BEFORE the import.
const dom = new JSDOM("<body></body>");
global.window = dom.window;
global.document = dom.window.document;
const mermaid = (await import("mermaid")).default;
mermaid.initialize({ startOnLoad: false });

const args = process.argv.slice(2);
const sources = args.length
  ? args.map((f) => [f, fs.readFileSync(f, "utf8")])
  : [["stdin", fs.readFileSync(0, "utf8")]];

let bad = 0;
for (const [name, text] of sources) {
  try {
    await mermaid.parse(text);
    console.log(`ok   ${name}`);
  } catch (e) {
    bad++;
    console.log(`FAIL ${name}\n${e.message}`);
  }
}
process.exit(bad ? 1 : 0);
