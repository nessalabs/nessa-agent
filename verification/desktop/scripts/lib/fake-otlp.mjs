/**
 * An OpenTelemetry collector, on this machine, for the stress check's
 * `telemetry` step: the gateway's `telemetry.otel.endpoint` names it, and it
 * keeps every OTLP/HTTP export it is sent, answering each as a collector
 * would.
 *
 * ```text
 *   POST /v1/traces   protobuf ExportTraceServiceRequest, kept whole; 200
 * ```
 *
 * The bodies are protobuf, whose strings are written as their bytes, so a
 * check asks whether an export holds a name or an id with `holds(...)`
 * without decoding it. `spans()` decodes just each span's ids and name, for
 * a check of which span sits under which.
 */
import { createHash } from "node:crypto"
import { createServer } from "node:http"

/**
 * The OTel ids the gateway's sink gives the span named by `trace` and `span`
 * keys (`nessa-telemetry`'s otel.rs: SHA-256 prefixes of the keys, each
 * followed by a zero byte), as hex.
 */
export function otelIds(trace, span) {
  const digest = (...parts) => {
    const hash = createHash("sha256")
    for (const part of parts) hash.update(part).update(Buffer.from([0]))
    return hash.digest()
  }
  return {
    traceId: digest(trace).subarray(0, 16).toString("hex"),
    spanId: digest(trace, span).subarray(0, 8).toString("hex"),
  }
}

/** The length-delimited and varint fields of one protobuf message. */
function* fields(bytes) {
  let at = 0
  const varint = () => {
    let value = 0
    let shift = 0
    for (;;) {
      if (at >= bytes.length) throw new Error("truncated protobuf")
      const byte = bytes[at++]
      value += (byte & 0x7f) * 2 ** shift
      if (byte < 0x80) return value
      shift += 7
    }
  }
  while (at < bytes.length) {
    const key = varint()
    const field = Math.floor(key / 8)
    switch (key & 7) {
      case 0:
        varint()
        break
      case 1:
        at += 8
        break
      case 2: {
        const length = varint()
        if (at + length > bytes.length) throw new Error("truncated protobuf")
        yield [field, bytes.subarray(at, at + length)]
        at += length
        break
      }
      case 5:
        at += 4
        break
      default:
        throw new Error(`unexpected protobuf wire type in field ${field}`)
    }
  }
}

/** Every span in one ExportTraceServiceRequest: ids as hex, and its name. */
function spansOf(body) {
  const spans = []
  for (const [resource, resourceSpans] of fields(body)) {
    if (resource !== 1) continue
    for (const [scope, scopeSpans] of fields(resourceSpans)) {
      if (scope !== 2) continue
      for (const [n, span] of fields(scopeSpans)) {
        if (n !== 2) continue
        const found = { traceId: "", spanId: "", parentSpanId: "", name: "" }
        for (const [f, value] of fields(span)) {
          if (f === 1) found.traceId = value.toString("hex")
          else if (f === 2) found.spanId = value.toString("hex")
          else if (f === 4) found.parentSpanId = value.toString("hex")
          else if (f === 5) found.name = value.toString("utf8")
        }
        spans.push(found)
      }
    }
  }
  return spans
}

/** Starts the fake on a free loopback port. Resolves with its controls. */
export async function startFakeOtlp() {
  /** Every export's body, in the order it came. */
  const bodies = []
  const server = createServer((request, response) => {
    const chunks = []
    request.on("data", (chunk) => chunks.push(chunk))
    request.on("end", () => {
      if (request.method === "POST" && request.url === "/v1/traces")
        bodies.push(Buffer.concat(chunks))
      response.writeHead(request.method === "POST" ? 200 : 404, {
        "content-type": "application/x-protobuf",
        "content-length": "0",
      })
      response.end()
    })
  })
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve))
  const { port } = server.address()
  return {
    /** The full traces URL for `telemetry.otel.endpoint`. */
    endpoint: `http://127.0.0.1:${port}/v1/traces`,
    /** How many exports came. */
    exports: () => bodies.length,
    /** Every span exported so far: `{traceId, spanId, parentSpanId, name}`. */
    spans: () => bodies.flatMap(spansOf),
    /** Whether one export holds every one of `texts`. */
    holds: (...texts) =>
      bodies.some((body) => texts.every((text) => body.includes(Buffer.from(text)))),
    /** Whether any export holds `text`. */
    anyHolds: (text) => bodies.some((body) => body.includes(Buffer.from(text))),
    close: () => new Promise((resolve) => server.close(() => resolve())),
  }
}
