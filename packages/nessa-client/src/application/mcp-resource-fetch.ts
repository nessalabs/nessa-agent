import { gatewayHttpOrigin } from "./gateway-http.js"

/** One redemption, as the route needs it. The ticket is a secret: never log it. */
export type McpResourceRequest = {
  /** Single-use ticket from `mcp.readResource`. */
  ticket: string
  /** Read no more of the body than this many bytes and one more. */
  maxBytes: number
  /** Abandons the request. The ticket may or may not have been spent. */
  signal?: AbortSignal
}

/**
 * What the resource route answered: its status, and for a `200` the body, cut
 * off one byte past `maxBytes`. Unchecked on purpose — whether those are the
 * bytes the ticket was issued for is decided by the caller of this port, in one
 * place, not by whichever adapter happened to fetch them.
 */
export type McpResourceReply = { status: number; bytes: Uint8Array<ArrayBuffer> }

/**
 * The resource route, as this package needs it. HTTP is on the other side.
 * An adapter rejects only when no answer arrived at all.
 */
export interface McpResourceTransport {
  get(request: McpResourceRequest): Promise<McpResourceReply>
}

/**
 * Why a resource's bytes were not handed back.
 *
 * - `not_found`: the route's one refusal (404) — the ticket is unknown, used,
 *   expired, released, or not one. Read the resource again for a new one.
 * - `unavailable`: the redemption could not be audited (503), so the bytes
 *   were not served.
 * - `integrity`: an answer whose bytes are not the ones described — their
 *   size or SHA-256 differs. Nothing is rendered from them.
 * - `aborted`: the caller's signal.
 * - `timeout`: no answer within the ticket's lifetime
 *   (`mcpAppDeadlines.fetchResourceMs`).
 * - `unreachable`: the request failed without an answer.
 * - `unexpected_response`: any other status.
 *
 * After any of these the ticket is spent or of unknown state.
 */
export type McpResourceFailureCode =
  | "not_found"
  | "unavailable"
  | "integrity"
  | "aborted"
  | "timeout"
  | "unreachable"
  | "unexpected_response"

/**
 * A resource whose bytes were not handed back, with a
 * {@link McpResourceFailureCode} to branch on. Nothing is retried for you. The
 * message and cause never contain the ticket.
 */
export class NessaMcpResourceError extends Error {
  constructor(
    /** Which refusal or fault this was. */
    readonly code: McpResourceFailureCode,
    /** HTTP status of the route's answer, when there was one. */
    readonly status?: number,
    cause?: unknown,
  ) {
    super(`MCP App resource was not fetched (${code})`, { cause })
    this.name = "NessaMcpResourceError"
  }
}

/** Where held resources are served for a session: `/mcp-resources` on the gateway's own HTTP origin. */
export function mcpResourceUrl(sessionUrl: string): string {
  return `${gatewayHttpOrigin(sessionUrl)}/mcp-resources`
}
