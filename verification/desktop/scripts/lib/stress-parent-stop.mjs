/** Provider evidence for the stress check's ordinary parent Stop. */

const object = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value)
const id = (value) =>
  value === null ||
  typeof value === "string" ||
  (typeof value === "number" && Number.isFinite(value))

// Admit the envelope needed to distinguish prompts from replies/notifications;
// this is not the gateway's complete ACP method/parameter decoder.
function envelope(frame) {
  if (!object(frame) || frame.jsonrpc !== "2.0") return false
  const method = Object.hasOwn(frame, "method")
  const result = Object.hasOwn(frame, "result")
  const error = Object.hasOwn(frame, "error")
  const identified = Object.hasOwn(frame, "id")
  if (identified && !id(frame.id)) return false
  return method
    ? typeof frame.method === "string" && frame.method.length > 0 && !result && !error
    : identified && result !== error && (!error || object(frame.error))
}

function records(recording) {
  if (recording === "") return []
  if (!recording.endsWith("\n")) throw new Error("incomplete ACP recording")
  return recording
    .split("\n")
    .filter(Boolean)
    .map((line) => {
      const record = JSON.parse(line)
      if (
        !object(record) ||
        !["to-agent", "from-agent"].includes(record.direction) ||
        !envelope(record.frame)
      )
        throw new Error("invalid ACP recording frame")
      if (
        record.frame.method === "session/prompt" &&
        (!object(record.frame.params) ||
          typeof record.frame.params.sessionId !== "string" ||
          record.frame.params.sessionId.length === 0)
      )
        throw new Error("ACP prompt has no native session identity")
      return record
    })
}

const isPrompt = ({ direction, frame }) =>
  direction === "to-agent" && frame.method === "session/prompt"

/**
 * The currently running parent's request identifies its native session. The
 * last exact request belongs to this engine, even when an earlier engine sent
 * the same stress text. Retain the complete prefix to detect a lost recording.
 */
export function parentStopCheckpoint(recording, text) {
  const request = records(recording).findLast(
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
  const dispatched = records(recording.slice(checkpoint.recording.length)).some(
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
