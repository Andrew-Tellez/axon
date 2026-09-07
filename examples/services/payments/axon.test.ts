// Las tres lineas que tejen el testkit generado con la implementacion real.
import { contractTests, machineTests } from "./axon.testkit.ts";
import { Payments } from "./index.ts";

contractTests((bus, inbox, outbox) => new Payments(bus, inbox, outbox, fakeDb()));
machineTests();

// La persistencia es de la persona, asi que el doble tambien.
function fakeDb() {
  const noop = async () => ({ rows: [], rowCount: 0 });
  return { query: noop, connect: async () => ({ query: noop, release() {} }) } as never;
}
