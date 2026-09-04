// OpenFeature de verdad, con el proveedor de flagd.
//
// La interfaz `Flags` que genera axon tiene la forma de OpenFeature a
// proposito: eso es todo lo que hace falta para que el SDK real encaje sin una
// capa de traduccion. axon no trae un SDK de flags igual que no trae uno de
// trazas — pone el nombre, el valor seguro y el campo por el que se fija, y el
// SDK lo elige cada equipo.
import { OpenFeature, type Client } from "@openfeature/server-sdk";
import { OFREPProvider } from "@openfeature/ofrep-provider";

// La misma forma que emite axon en cada servicio. Igual que en runtime.ts, se
// declara aca porque un modulo compartido no puede importar un contrato que es
// por servicio; el tipado estructural hace que encajen.
export interface Flags {
  evaluar<T extends boolean | string | number | object>(
    nombre: string,
    porDefecto: T,
    contexto: Record<string, string>,
  ): Promise<T>;
}

let cliente: Client | undefined;

export async function arrancarFlags() {
  const url = process.env.AXON_FLAGS_URL;
  if (!url) return;
  // OFREP y no el proveedor de flagd a proposito: OFREP es el protocolo REST
  // estandar de OpenFeature, asi que esto habla con flagd hoy y con cualquier
  // otro backend que lo implemente sin cambiar una linea. El proveedor gRPC de
  // flagd, ademas, pide la ruta vieja del servicio de evaluacion, y flagd v0.12
  // ya sirve solo la nueva: la prueba real lo encontro.
  await OpenFeature.setProviderAndWait(new OFREPProvider({ baseUrl: url }));
  cliente = OpenFeature.getClient(process.env.AXON_SERVICE ?? "axon");
}

/** Implementacion de la interfaz que genera axon.
 *
 *  Sin proveedor devuelve el valor por defecto que declaro el manifiesto: un
 *  flagd caido no debe cambiar el comportamiento, y el valor seguro ya esta
 *  elegido en la declaracion. */
export const flags: Flags = {
  async evaluar(nombre, porDefecto, contexto) {
    if (!cliente) return porDefecto;
    // OpenFeature resuelve un tipo por flag, asi que el accesor correcto sale
    // del tipo del valor por defecto —que el manifiesto ya declaro.
    switch (typeof porDefecto) {
      case "boolean":
        return (await cliente.getBooleanValue(nombre, porDefecto, contexto)) as typeof porDefecto;
      case "string":
        return (await cliente.getStringValue(nombre, porDefecto, contexto)) as typeof porDefecto;
      case "number":
        return (await cliente.getNumberValue(nombre, porDefecto, contexto)) as typeof porDefecto;
      default:
        return (await cliente.getObjectValue(nombre, porDefecto as never, contexto)) as typeof porDefecto;
    }
  },
};
