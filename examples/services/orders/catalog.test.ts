// The half that pays for declaring the list: a value that is not on it does
// not compile. `tsc --strict` over this file is the check — an assertion in a
// test would only prove that the array has three entries, which is not the
// claim.
import { findCurrency, currencyCatalog, type Currency } from "./contracts.ts";
import test from "node:test";
import assert from "node:assert/strict";

test("the catalog is the declared list", () => {
  const mxn: Currency = "MXN";
  assert.equal(findCurrency(mxn).decimals, 2);
  // @ts-expect-error a currency that is not in the catalog does not compile
  const invented: Currency = "XYZ";
  void invented;
  assert.equal(currencyCatalog.length, 3);
  // frozen: a catalog somebody can mutate at runtime is a catalog that stops
  // matching the table
  assert.equal(Object.isFrozen(currencyCatalog), true);
});
