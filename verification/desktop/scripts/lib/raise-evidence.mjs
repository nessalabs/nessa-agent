/**
 * What the agent's wire says of one raised turn (`conversation-stress.mjs`'s
 * `raise` step), read from the ACP recording the stress stack keeps: the
 * frames written after `before`, the recording as it stood before the turn
 * was sent.
 *
 * A raise reaches the running turn when the mode change is sent while the
 * turn's prompt is still unanswered, after the review it raised over was
 * asked and before that review is answered: then the review is answered after
 * the change, and the turn asks for nothing more.
 *
 * ```text
 * session/prompt (the turn) ─▶ request_permission (open) ─▶ set_config_option (mode)
 *   ─▶ that permission answered ─▶ … ─▶ the prompt's result
 * ```
 */

/** Each frame of `recording` after its first `before` characters, in order. */
function framesAfter(before, recording) {
  return recording
    .slice(before.length)
    .split("\n")
    .filter((line) => line.trim() !== "")
    .flatMap((line) => {
      try {
        const { direction, frame } = JSON.parse(line)
        return frame ? [{ direction, frame }] : []
      } catch {
        return []
      }
    })
}

/**
 * The raised turn's order on the wire, for the prompt whose text includes
 * `promptText`: `{ asked, modeChanged, answeredAfterChange, endedAfterChange }`
 * — how many reviews the turn asked, whether a mode change was sent while it
 * ran, whether the review open at that change was answered after it, and
 * whether the turn ended after it. `null` when the prompt is not in the frames.
 */
export function raisedTurn(before, recording, promptText) {
  const frames = framesAfter(before, recording)
  const start = frames.findIndex(
    ({ direction, frame }) =>
      direction === "to-agent" &&
      frame.method === "session/prompt" &&
      JSON.stringify(frame.params?.prompt ?? []).includes(promptText),
  )
  if (start < 0) return null
  const promptId = frames[start].frame.id
  const sessionId = frames[start].frame.params?.sessionId
  const end = frames.findIndex(
    ({ direction, frame }, index) =>
      index > start &&
      direction === "from-agent" &&
      frame.id === promptId &&
      frame.method === undefined,
  )
  const during = frames.slice(start + 1, end < 0 ? undefined : end)
  const asks = during.filter(
    ({ direction, frame }) =>
      direction === "from-agent" &&
      frame.method === "session/request_permission" &&
      frame.params?.sessionId === sessionId,
  )
  const change = during.findIndex(
    ({ direction, frame }) =>
      direction === "to-agent" &&
      frame.method === "session/set_config_option" &&
      frame.params?.sessionId === sessionId &&
      frame.params?.configId === "mode",
  )
  const open = asks[0]
  const answered =
    open === undefined
      ? -1
      : during.findIndex(
          ({ direction, frame }) =>
            direction === "to-agent" &&
            frame.id === open.frame.id &&
            frame.method === undefined,
        )
  const askedAt = open === undefined ? -1 : during.indexOf(open)
  return {
    asked: asks.length,
    modeChanged: change >= 0 && askedAt >= 0 && change > askedAt,
    answeredAfterChange: change >= 0 && answered > change,
    endedAfterChange: change >= 0 && end >= 0,
  }
}
