// Real OpenFeature, with the flagd provider.
//
// The `Flags` interface axon generates has OpenFeature's shape on purpose: that
// is all it takes for the real SDK to fit with no translation layer. axon ships
// no flag SDK just as it ships none for traces — it provides the name, the safe
// value and the field it is pinned by, and the SDK is each team's choice.
import { OpenFeature, type Client } from "@openfeature/server-sdk";
import { OFREPProvider } from "@openfeature/ofrep-provider";

// The same shape axon emits in every service. As in runtime.ts, it is declared
// here because a shared module cannot import a contract that is per service;
// structural typing makes them fit.
export interface Flags {
  evaluate<T extends boolean | string | number | object>(
    name: string,
    fallback: T,
    context: Record<string, string>,
  ): Promise<T>;
}

let client: Client | undefined;

export async function startFlags() {
  const url = process.env.AXON_FLAGS_URL;
  if (!url) return;
  // OFREP and not flagd's own provider, on purpose: OFREP is OpenFeature's
  // standard REST protocol, so this talks to flagd today and to any other
  // backend that implements it without changing a line. flagd's gRPC provider,
  // on top of that, asks for the old evaluation-service path, and flagd v0.12
  // already serves only the new one: the real test found that out.
  await OpenFeature.setProviderAndWait(new OFREPProvider({ baseUrl: url }));
  client = OpenFeature.getClient(process.env.AXON_SERVICE ?? "axon");
}

/** An implementation of the interface axon generates.
 *
 *  With no provider it returns the default value the manifest declared: a flagd
 *  that is down must not change the behaviour, and the safe value has already
 *  been chosen in the declaration. */
export const flags: Flags = {
  async evaluate(name, fallback, context) {
    if (!client) return fallback;
    // OpenFeature resolves one type per flag, so the right accessor comes from
    // the type of the default value —which the manifest already declared.
    switch (typeof fallback) {
      case "boolean":
        return (await client.getBooleanValue(name, fallback, context)) as typeof fallback;
      case "string":
        return (await client.getStringValue(name, fallback, context)) as typeof fallback;
      case "number":
        return (await client.getNumberValue(name, fallback, context)) as typeof fallback;
      default:
        return (await client.getObjectValue(name, fallback as never, context)) as typeof fallback;
    }
  },
};
