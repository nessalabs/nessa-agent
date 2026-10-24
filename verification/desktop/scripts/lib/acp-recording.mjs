/** Strict complete recorder evidence shared by stress oracles. */

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

export function acpRecords(recording) {
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
