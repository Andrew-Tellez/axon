// The three lines that weave the generated testkit into the real implementation.
import { contractTests, machineTests } from "./axon.testkit.ts";
import { Payments } from "./index.ts";

contractTests((bus, inbox, outbox) => new Payments(bus, inbox, outbox, fakeDb()));
machineTests();

// The persistence belongs to the person, so the double does too.
function fakeDb() {
  const noop = async () => ({ rows: [], rowCount: 0 });
  return { query: noop, connect: async () => ({ query: noop, release() {} }) } as never;
}
