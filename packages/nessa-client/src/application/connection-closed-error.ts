import { SessionCloseReason, sessionClosePolicy } from "../generated/product.js"

/** Typed termination of one transport. A failed RPC may already have executed. */
export class NessaConnectionClosedError extends Error {
  readonly closeReason: SessionCloseReason | "unknown"
  readonly retryable: boolean
  readonly retryAfterMs?: number

  constructor(
    readonly code: number,
    readonly reason: string,
  ) {
    super(`WebSocket closed (${code})`)
    this.name = "NessaConnectionClosedError"
    const fallback = Object.entries(sessionClosePolicy).find(
      ([, policy]) => policy.webSocketCode === code,
    )?.[0] as SessionCloseReason | undefined
    this.closeReason =
      fallback ??
      ([1001, 1011].includes(code) ? SessionCloseReason.TransportInterrupted : "unknown")
    this.retryable =
      this.closeReason !== "unknown" && sessionClosePolicy[this.closeReason].retryable
    try {
      const payload = JSON.parse(reason)
      // Codes are authoritative: arbitrary server hints cannot make revocation retryable.
      if (
        payload.code === this.closeReason &&
        payload.retryable === this.retryable &&
        this.retryable &&
        Number.isSafeInteger(payload.retryAfterMs) &&
        payload.retryAfterMs >= 0
      ) {
        this.retryAfterMs = Math.min(payload.retryAfterMs, 60_000)
      }
    } catch {
      /* Empty and legacy close reasons use the numeric-code policy. */
    }
  }
}
