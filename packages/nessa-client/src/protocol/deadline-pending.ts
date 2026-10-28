/**
 * A moment on the gateway's clock — a deadline's `dueAtMs`, a usage limit's
 * reset, a tool call's start — as the same moment on this client's clock: as
 * long after `receivedAtMs` as it was after the view's `serverNowMs`.
 *
 * The gateway and the client are often different machines, and their clocks
 * need not agree. Measured against the gateway's own time in the same view,
 * the time left does not move with the difference between the two clocks;
 * what is left over is how long the view took to arrive. A client calls this
 * once, as the view is received, with its own clock's reading then; what it
 * returns is what {@link deadlinePendingLine} counts down to on that clock.
 */
export function clientDueAtMs(
  dueAtMs: number,
  serverNowMs: number,
  receivedAtMs: number,
): number {
  return receivedAtMs + (dueAtMs - serverNowMs)
}

/**
 * The sentence a pending review shows while its deadline is still armed.
 *
 * Built from the outcome and the time left until `dueAtMs`, so a reloaded
 * client does not start `waitMs` over. The past-tense `reason` stays on the
 * notice and the audit; this line is what has not happened yet. `dueAtMs` and
 * `nowMs` are on the same clock, this client's: a view's own `dueAtMs` is on
 * the gateway's and goes through {@link clientDueAtMs} first.
 */
export function deadlinePendingLine(
  deadline: {
    readonly dueAtMs: number
    readonly effect: "allow" | "deny"
    readonly stopsTurn: boolean
  },
  nowMs: number,
): string {
  const outcome =
    deadline.effect === "allow" ? "allowed automatically" : "denied automatically"
  const stop = deadline.stopsTurn ? " and the agent stops" : ""
  const remainingMs = deadline.dueAtMs - nowMs
  if (!Number.isFinite(remainingMs) || remainingMs <= 0)
    return `If nobody answers, this is ${outcome}${stop}.`
  const seconds = Math.ceil(remainingMs / 1000)
  // Past a minute, whole minutes: "within 5 minutes", not "within 300 seconds".
  const [count, unit] =
    seconds > 60 ? [Math.ceil(seconds / 60), "minute"] : [seconds, "second"]
  return `If nobody answers within ${count} ${unit}${count === 1 ? "" : "s"}, this is ${outcome}${stop}.`
}
