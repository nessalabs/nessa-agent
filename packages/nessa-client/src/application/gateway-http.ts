/**
 * The gateway's HTTP routes beside its socket: `PUT /attachments` and
 * `GET /mcp-resources`. Bytes never ride the socket, so each travels on a
 * request of its own, authorised by a ticket the socket issued. What they share
 * lives here once: where they are, the clock their deadlines run on, and how
 * one request ends.
 */

/**
 * The clock a request's deadline runs on: call `elapsed` once after `ms`,
 * unless the returned function is called first. Composition supplies
 * `setTimeout`; tests supply one they fire by hand, so no test waits.
 */
export type RequestTimer = (ms: number, elapsed: () => void) => () => void

/**
 * Where a session's HTTP routes are: the gateway's own origin. Same host and
 * port as the WebSocket, `ws` to `http` and `wss` to `https`, and no path —
 * `/session` and `/browser/session` are socket routes.
 */
export function gatewayHttpOrigin(sessionUrl: string): string {
  const url = new URL(sessionUrl)
  if (url.protocol !== "ws:" && url.protocol !== "wss:")
    throw new TypeError("Session URL must use ws: or wss:")
  return `${url.protocol === "wss:" ? "https:" : "http:"}//${url.host}`
}

/** The errors one request can end in other than its answer, each made by its route. */
export type RequestEndings = {
  /** The caller's signal aborted it. */
  aborted: () => Error
  /** No answer within the deadline. */
  timeout: () => Error
  /** The request failed without an answer. */
  unreachable: (cause: unknown) => Error
}

/**
 * One request that ends for exactly one reason: an answer, the caller's signal,
 * or the deadline. The request is aborted for the last two, and the wait ends
 * even if the transport ignores the abort — a request that never answers is the
 * case the deadline exists for, and it may not answer an abort either.
 */
export function requestWithin<T>(
  send: (signal: AbortSignal) => Promise<T>,
  caller: AbortSignal | undefined,
  timer: RequestTimer,
  deadlineMs: number,
  endings: RequestEndings,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const request = new AbortController()
    let settled = false
    // Replaced the moment the timer is armed. A timer may spend its whole budget
    // before returning a handle, and then the deadline settles this while there
    // is still nothing to stop; the handle is used below instead.
    let stopTimer = () => {}
    const finish = (settle: () => void) => {
      if (settled) return
      settled = true
      stopTimer()
      caller?.removeEventListener("abort", onCallerAbort)
      settle()
    }
    const stop = (error: Error) =>
      finish(() => {
        request.abort()
        reject(error)
      })
    const onCallerAbort = () => stop(endings.aborted())
    stopTimer = timer(deadlineMs, () => stop(endings.timeout()))
    // Already out of time before the request was made: nothing to send.
    if (settled) return stopTimer()
    if (caller?.aborted) return onCallerAbort()
    caller?.addEventListener("abort", onCallerAbort, { once: true })
    send(request.signal).then(
      (reply) => finish(() => resolve(reply)),
      (cause: unknown) => finish(() => reject(endings.unreachable(cause))),
    )
  })
}
