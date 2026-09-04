// Las tres lineas que tejen el testkit generado con la implementacion real.
import { pruebasDeContrato, pruebasDeMaquinas } from "./axon.testkit.ts";
import { Payments } from "./index.ts";

pruebasDeContrato((bus, inbox, outbox) => new Payments(bus, inbox, outbox, fakeDb()));
pruebasDeMaquinas();

// La persistencia es de la persona, asi que el doble tambien.
function fakeDb() {
  const noop = async () => ({ rows: [], rowCount: 0 });
  return { query: noop, connect: async () => ({ query: noop, release() {} }) } as never;
}
