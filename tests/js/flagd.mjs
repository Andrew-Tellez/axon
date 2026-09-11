// Validates a flagd configuration against flagd's OWN schema.
//
// The rule of this suite is that a generator is verified with the real tool of
// its ecosystem. flagd has no linter to shell out to, but it publishes the
// schema, and a JSON that parses is not a configuration flagd accepts: a
// variant that is not in `variants`, a `defaultVariant` that names nothing or
// a state that is not `ENABLED`/`DISABLED` are all valid JSON and a provider
// that starts empty — every flag reading its code default, which is the
// failure that looks like nothing happening.
//
// The schema is VENDORED, in `schemas/`, and not fetched: a test that needs
// the network to have an opinion has no opinion on a plane, and a schema that
// changes under the test turns a red build into a mystery. Refresh it with
//
//   curl -o tests/js/schemas/flagd-flags.json https://flagd.dev/schema/v0/flags.json
//   curl -o tests/js/schemas/flagd-targeting.json https://flagd.dev/schema/v0/targeting.json
//
// By hand:  axon flags . | node tests/js/flagd.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import Ajv from "ajv";
import addFormats from "ajv-formats";

const here = path.dirname(fileURLToPath(import.meta.url));
const read = (f) => JSON.parse(fs.readFileSync(path.join(here, "schemas", f), "utf8"));

const ajv = new Ajv({ strict: false, allErrors: true });
addFormats(ajv);
// the flags schema points at the targeting one by URL; it is given by id so
// nothing reaches for the network
ajv.addSchema(read("flagd-targeting.json"), "https://flagd.dev/schema/v0/targeting.json");
const validate = ajv.compile(read("flagd-flags.json"));

const args = process.argv.slice(2);
const sources = args.length
  ? args.map((f) => [f, fs.readFileSync(f, "utf8")])
  : [["stdin", fs.readFileSync(0, "utf8")]];

let bad = 0;
for (const [name, text] of sources) {
  if (validate(JSON.parse(text))) {
    console.log(`ok   ${name}`);
  } else {
    bad++;
    console.log(`FAIL ${name}`);
    for (const e of validate.errors) console.log(`  ${e.instancePath || "/"} ${e.message}`);
  }
}
process.exit(bad ? 1 : 0);
