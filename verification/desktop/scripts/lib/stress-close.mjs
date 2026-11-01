/** Close-thread acceptance and bounded, credential-free parking evidence. */
import { acpRecords } from "./acp-recording.mjs"

const uuid = (value) =>
  typeof value === "string" &&
  /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i.test(value)
    ? value
    : null

/** Only the exact prompt's session and its matching tool receipt can accept. */
export function closeAcceptance(recording, text, conversationId) {
  const records = acpRecords(recording)
  const start = records.findLastIndex(
    ({ direction, frame }) =>
      direction === "to-agent" &&
      frame.method === "session/prompt" &&
      frame.params.prompt?.some((part) => part.type === "text" && part.text === text),
  )
  const missing = { state: "unconfirmed", threadId: null, closing: null }
  if (start < 0) return missing
  const session = records[start].frame.params.sessionId
  const following = records.slice(start + 1)
  const next = following.findIndex(
    ({ direction, frame }) =>
      direction === "to-agent" &&
      frame.method === "session/prompt" &&
      frame.params.sessionId === session,
  )
  const updates = (next < 0 ? following : following.slice(0, next))
    .filter(
      ({ direction, frame }) =>
        direction === "from-agent" &&
        frame.method === "session/update" &&
        frame.params?.sessionId === session,
    )
    .map(({ frame }) => frame.params.update)
  const calls = updates.filter(
    (update) =>
      update?.sessionUpdate === "tool_call" &&
      typeof update.toolCallId === "string" &&
      update.toolCallId.length > 0 &&
      update.title === "close_thread" &&
      (update._meta?.claudeCode?.toolName === "mcp__nessa-agents__close_thread" ||
        (update.rawInput?.server === "nessa-agents" &&
          update.rawInput?.tool === "close_thread")),
  )
  if (calls.length !== 1) return missing
  const results = updates.filter(
    (update) =>
      update?.sessionUpdate === "tool_call_update" &&
      update.toolCallId === calls[0].toolCallId &&
      ["completed", "failed"].includes(update.status),
  )
  // The fixture emits one terminal receipt after its call. Never replace a
  // refusal with a later success or borrow an answer that preceded the call.
  if (results.length !== 1 || updates.indexOf(results[0]) <= updates.indexOf(calls[0]))
    return missing
  const result = results[0]
  if (result.status === "failed") return { ...missing, state: "refused" }
  const parts = result.content?.filter(
    (part) => part.type === "content" && part.content?.type === "text",
  )
  if (parts?.length !== 1) return missing
  let value
  try {
    value = JSON.parse(parts[0].content.text)
  } catch {
    return missing
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return missing
  const threadId = uuid(value.threadId)
  const closing = typeof value.closing === "boolean" ? value.closing : null
  const state =
    Object.hasOwn(value, "error") || closing === false
      ? "refused"
      : closing === true && threadId === conversationId
        ? "accepted"
        : closing === true && threadId !== null
          ? "wrong-thread"
          : "unconfirmed"
  // Do not return provider text, arbitrary errors, or other payload fields.
  return { state, threadId, closing }
}

/** Read the step's actual target; most steps still act on the main conversation. */
export function readFailureView(run, read) {
  return read(run.client, run.failureConversationId ?? run.main.id)
}

/** Project known log markers only; raw log lines may carry credentials. */
export function parkingEvidence(log, offset, conversationId) {
  const markers = []
  let position = 0
  for (const line of log.split("\n")) {
    const after = position >= offset
    position += line.length + 1
    const event = line.includes(
      "the agent closed its thread; it stops once its turn ends",
    )
      ? "accepted"
      : line.includes("an idle agent's stop did not confirm cleanup")
        ? "cleanup-failed"
        : line.includes("parking an idle agent")
          ? "parking"
          : null
    if (!event) continue
    const threadId = uuid(/conversation_id=([a-f0-9-]+)/i.exec(line)?.[1])
    if (!threadId) continue
    markers.push({
      event,
      threadId,
      at: /\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z/.exec(line)?.[0] ?? null,
      after,
    })
  }
  const current = markers.filter((marker) => marker.after)
  return {
    accepted: current.some(
      (marker) => marker.event === "accepted" && marker.threadId === conversationId,
    ),
    parking: current.filter((marker) => marker.event === "parking").length,
    targetParking: current.filter(
      (marker) => marker.event === "parking" && marker.threadId === conversationId,
    ).length,
    cleanupFailures: current.filter((marker) => marker.event === "cleanup-failed").length,
    previousParking:
      markers.findLast((marker) => !marker.after && marker.event === "parking") ?? null,
    recent: current.slice(-12),
  }
}
