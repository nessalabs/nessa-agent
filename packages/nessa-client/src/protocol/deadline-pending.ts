/**
 * The sentence a pending review shows while its deadline is still armed.
 *
 * Built from the outcome and the time left until `dueAtMs`, so a reloaded
 * client does not start `waitMs` over. The past-tense `reason` stays on the
 * notice and the audit; this line is what has not happened yet.
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
  const unit = seconds === 1 ? "second" : "seconds"
  return `If nobody answers within ${seconds} ${unit}, this is ${outcome}${stop}.`
}
