// OpenTelemetry in the example, not in axon: the framework brings up the
// backend and sets the standard variables; the SDK is each team's choice.
//
// The interesting part is that there is nothing to wire between the two. The
// envelope already carries `traceparent`, which IS the W3C context OTel
// propagates, so a span created from it continues the same trace even when the
// other end is in another language.
import { NodeSDK } from "@opentelemetry/sdk-node";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { trace, context, propagation, SpanStatusCode, type Span } from "@opentelemetry/api";
import type { Envelope } from "./runtime.ts";

// The SDK reads OTEL_SERVICE_NAME, OTEL_EXPORTER_OTLP_ENDPOINT and
// OTEL_RESOURCE_ATTRIBUTES from the environment: that is what `axon infra`
// injects.
export function startTelemetry() {
  if (!process.env.OTEL_EXPORTER_OTLP_ENDPOINT) return;
  const sdk = new NodeSDK({ traceExporter: new OTLPTraceExporter() });
  sdk.start();
  const stop = () => void sdk.shutdown().catch(() => {});
  process.once("SIGTERM", stop);
  process.once("SIGINT", stop);
}

const tracer = () => trace.getTracer("axon");

function traceparentOf(c: { traceId: string; spanId: string; traceFlags: number }) {
  const flags = (c.traceFlags & 1) === 1 ? "01" : "00";
  return `00-${c.traceId}-${c.spanId}-${flags}`;
}

const validId = (t: string) => !!t && !/^0+$/.test(t);

/** Annotates the active span. The edge creates the envelope inside the span, so
 *  its ids get added after opening it. */
export function annotate(attrs: Record<string, string>) {
  const span = trace.getSpan(context.active());
  for (const [k, v] of Object.entries(attrs)) span?.setAttribute(k, v);
}

/** The producer's span, and it rewrites the envelope's `traceparent` with its
 *  own.
 *
 *  It is needed because `newEnvelope` generates a synthetic span-id: it does not
 *  know OTel, so it invents one to represent "the span that produced this
 *  message". Sent like that, the consumer extracts a parent that never existed
 *  and the trace ends up in fragments. Here the producer puts its real span in
 *  before sending.
 *
 *  The parent is the active span if there is one —the handler that emits— and if
 *  not, the envelope's own: that last case is the outbox relay, which publishes
 *  outside the context of whoever staged it. */
export async function inProducer<T>(
  name: string,
  e: Envelope<unknown>,
  attrs: Record<string, string>,
  fn: () => Promise<T>,
): Promise<T> {
  const active = trace.getSpan(context.active())?.spanContext();
  const parent = active && validId(active.traceId)
    ? context.active()
    : propagation.extract(context.active(), { traceparent: e.traceparent });
  return tracer().startActiveSpan(name, {}, parent, async (span) => {
    for (const [k, v] of Object.entries(attrs)) span.setAttribute(k, v);
    span.setAttribute("messaging.message.id", e.id);
    span.setAttribute("axon.correlation_id", e.correlationId);
    if (e.causationId) span.setAttribute("axon.causation_id", e.causationId);
    const c = span.spanContext();
    if (validId(c.traceId)) e.traceparent = traceparentOf(c);
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

/** Opens the edge's span and hands `fn` that span's real `traceparent`, so the
 *  envelope inherits it. `incomingParent`, when present, continues it. */
export async function atEdge<T>(
  name: string,
  incomingParent: string | undefined,
  attrs: Record<string, string>,
  fn: (traceparent: string) => Promise<T>,
): Promise<T> {
  const parent = incomingParent
    ? propagation.extract(context.active(), { traceparent: incomingParent })
    : context.active();
  return tracer().startActiveSpan(name, {}, parent, async (span) => {
    for (const [k, v] of Object.entries(attrs)) span.setAttribute(k, v);
    const c = span.spanContext();
    // with no SDK active the ids are zeros: there one gets generated, as before
    const valid = validId(c.traceId);
    const tp = valid
      ? traceparentOf(c)
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

/** Runs `fn` in a span that is a child of the envelope. The trace crosses the
 *  process because the parent comes in the message's `traceparent`, not from a
 *  global variable. */
export async function inSpan<T>(
  name: string,
  e: Envelope<unknown> | undefined,
  attrs: Record<string, string>,
  fn: (span: Span) => Promise<T>,
): Promise<T> {
  const parent = e
    ? propagation.extract(context.active(), { traceparent: e.traceparent })
    : context.active();
  return tracer().startActiveSpan(name, {}, parent, async (span) => {
    for (const [k, v] of Object.entries(attrs)) span.setAttribute(k, v);
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
