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
 * without decoding it.
 */
import { createServer } from "node:http"

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
    /** Whether one export holds every one of `texts`. */
    holds: (...texts) =>
      bodies.some((body) => texts.every((text) => body.includes(Buffer.from(text)))),
    /** Whether any export holds `text`. */
    anyHolds: (text) => bodies.some((body) => body.includes(Buffer.from(text))),
    close: () => new Promise((resolve) => server.close(() => resolve())),
  }
}
