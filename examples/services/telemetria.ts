// OpenTelemetry en el ejemplo, no en axon: el framework levanta el backend y
// pone las variables estandar; el SDK lo elige cada equipo.
//
// El punto interesante es que no hay nada que cablear entre los dos. El
// envelope ya lleva `traceparent`, que ES el contexto W3C que propaga OTel,
// asi que un span creado a partir de el continua la misma traza aunque el
// otro extremo este en otro lenguaje.
import { NodeSDK } from "@opentelemetry/sdk-node";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { trace, context, propagation, SpanStatusCode, type Span } from "@opentelemetry/api";
import type { Envelope } from "./runtime.ts";

// El SDK lee OTEL_SERVICE_NAME, OTEL_EXPORTER_OTLP_ENDPOINT y
// OTEL_RESOURCE_ATTRIBUTES del entorno: eso es lo que inyecta `axon infra`.
export function arrancarTelemetria() {
  if (!process.env.OTEL_EXPORTER_OTLP_ENDPOINT) return;
  const sdk = new NodeSDK({ traceExporter: new OTLPTraceExporter() });
  sdk.start();
  const apagar = () => void sdk.shutdown().catch(() => {});
  process.once("SIGTERM", apagar);
  process.once("SIGINT", apagar);
}

const rastreador = () => trace.getTracer("axon");

function traceparentDe(c: { traceId: string; spanId: string; traceFlags: number }) {
  const flags = (c.traceFlags & 1) === 1 ? "01" : "00";
  return `00-${c.traceId}-${c.spanId}-${flags}`;
}

const idValido = (t: string) => !!t && !/^0+$/.test(t);

/** Anota el span activo. El borde crea el envelope dentro del span, asi que
 *  sus ids se agregan despues de abrirlo. */
export function anotar(atributos: Record<string, string>) {
  const span = trace.getSpan(context.active());
  for (const [k, v] of Object.entries(atributos)) span?.setAttribute(k, v);
}

/** Span del productor, y reescribe el `traceparent` del envelope con el suyo.
 *
 *  Hace falta porque `newEnvelope` genera un span-id sintetico: no conoce OTel,
 *  asi que inventa uno para representar "el span que produjo este mensaje". Si
 *  se envia asi, el consumidor extrae un padre que nunca existio y la traza
 *  queda en fragmentos. Aqui el productor pone su span real antes de enviar.
 *
 *  El padre es el span activo si lo hay —el handler que emite— y si no, el del
 *  propio envelope: eso ultimo es el caso del relay del outbox, que publica
 *  fuera del contexto de quien lo guardo. */
export async function enProductor<T>(
  nombre: string,
  e: Envelope<unknown>,
  atributos: Record<string, string>,
  fn: () => Promise<T>,
): Promise<T> {
  const activo = trace.getSpan(context.active())?.spanContext();
  const padre = activo && idValido(activo.traceId)
    ? context.active()
    : propagation.extract(context.active(), { traceparent: e.traceparent });
  return rastreador().startActiveSpan(nombre, {}, padre, async (span) => {
    for (const [k, v] of Object.entries(atributos)) span.setAttribute(k, v);
    span.setAttribute("messaging.message.id", e.id);
    span.setAttribute("axon.correlation_id", e.correlationId);
    if (e.causationId) span.setAttribute("axon.causation_id", e.causationId);
    const c = span.spanContext();
    if (idValido(c.traceId)) e.traceparent = traceparentDe(c);
    try {
      return await fn();
    } catch (err) {
      span.setStatus({ code: SpanStatusCode.ERROR, message: String(err) });
      throw err;
    } finally {
      span.end();
    }
  });
}

/** Abre el span del borde y le pasa a `fn` el `traceparent` real de ese span,
 *  para que el envelope lo herede. `padreEntrante`, si viene, lo continua. */
export async function enBorde<T>(
  nombre: string,
  padreEntrante: string | undefined,
  atributos: Record<string, string>,
  fn: (traceparent: string) => Promise<T>,
): Promise<T> {
  const padre = padreEntrante
    ? propagation.extract(context.active(), { traceparent: padreEntrante })
    : context.active();
  return rastreador().startActiveSpan(nombre, {}, padre, async (span) => {
    for (const [k, v] of Object.entries(atributos)) span.setAttribute(k, v);
    const c = span.spanContext();
    // sin SDK activo los ids son ceros: ahi se genera uno, como antes
    const valido = idValido(c.traceId);
    const tp = valido
      ? traceparentDe(c)
      : `00-${crypto.randomUUID().replace(/-/g, "")}-${crypto.randomUUID().replace(/-/g, "").slice(0, 16)}-01`;
    try {
      return await fn(tp);
    } catch (err) {
      span.setStatus({ code: SpanStatusCode.ERROR, message: String(err) });
      throw err;
    } finally {
      span.end();
    }
  });
}

/** Ejecuta `fn` en un span hijo del envelope. La traza cruza el proceso porque
 *  el padre viene en el `traceparent` del mensaje, no de una variable global. */
export async function enSpan<T>(
  nombre: string,
  e: Envelope<unknown> | undefined,
  atributos: Record<string, string>,
  fn: (span: Span) => Promise<T>,
): Promise<T> {
  const padre = e
    ? propagation.extract(context.active(), { traceparent: e.traceparent })
    : context.active();
  return rastreador().startActiveSpan(nombre, {}, padre, async (span) => {
    for (const [k, v] of Object.entries(atributos)) span.setAttribute(k, v);
    if (e) {
      span.setAttribute("messaging.message.id", e.id);
      span.setAttribute("axon.correlation_id", e.correlationId);
      if (e.causationId) span.setAttribute("axon.causation_id", e.causationId);
    }
    try {
      return await fn(span);
    } catch (err) {
      span.setStatus({ code: SpanStatusCode.ERROR, message: String(err) });
      throw err;
    } finally {
      span.end();
    }
  });
}
