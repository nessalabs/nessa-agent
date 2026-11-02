/** Provider evidence for the stress check's ordinary parent Stop. */

import { acpRecords } from "./acp-recording.mjs"

/** Compare projected origin identity; matching words convey no authority. */
export function hasAgentSource(turn, source) {
  return (
    typeof source?.conversationId === "string" &&
    source.conversationId.length > 0 &&
    typeof source?.executionId === "string" &&
    source.executionId.length > 0 &&
    turn?.agent?.conversationId === source.conversationId &&
    turn?.agent?.executionId === source.executionId
  )
}

const isPrompt = ({ direction, frame }) =>
  direction === "to-agent" && frame.method === "session/prompt"

/**
 * The currently running parent's request identifies its native session. The
 * last exact request belongs to this engine, even when an earlier engine sent
 * the same stress text. Retain the complete prefix to detect a lost recording.
 */
export function parentStopCheckpoint(recording, text) {
  const request = acpRecords(recording).findLast(
    (record) =>
      isPrompt(record) &&
      record.frame.params?.prompt?.some(
        (part) => part.type === "text" && part.text === text,
      ),
  )
  const sessionId = request?.frame.params?.sessionId
  if (typeof sessionId !== "string" || !sessionId)
    throw new Error("running parent's prompt missing from ACP recording")
  return { recording, sessionId }
}

/**
 * A queued report can acquire cancelled history when Stop withdraws it. Check
 * actual provider requests as well as history: even a request cancelled with
 * no assistant output is a restart, and another conversation's work is not.
 */
export function parentRestarted(checkpoint, recording, before, after) {
  if (!recording.startsWith(checkpoint.recording))
    throw new Error("ACP recording changed before the Stop checkpoint")
  const dispatched = acpRecords(recording.slice(checkpoint.recording.length)).some(
    (record) =>
      isPrompt(record) && record.frame.params?.sessionId === checkpoint.sessionId,
  )
  return (
    dispatched ||
    after.some(
      (turn) =>
        !before.has(turn.executionId) &&
        (turn.status !== "cancelled" || turn.parts.length !== 0),
    )
  )
}
