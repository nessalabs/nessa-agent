import type { AgentEvent, JsonValue } from "@nessalabs/agent-stream"
import {
  isToolGroup,
  type Transcript,
  type Turn,
} from "@nessalabs/agent-stream/transcript"

/** Display fields derived from the shared transcript, including retained raw snapshot facts. */
export type AgentToolView = {
  callId: string
  title: string
  status: string
  input: string
  details: string
}
function rawText(raw: JsonValue, key: string): string {
  if (raw === null || Array.isArray(raw) || typeof raw !== "object") return ""
  return typeof raw[key] === "string" ? raw[key] : ""
}
export function agentTurnView(turn: Turn, transcript: Transcript) {
  const events: AgentEvent[] = turn.work.flatMap((item) =>
    isToolGroup(item) ? [...item.calls] : [item],
  )
  // The builder separates finalText from work. Find the event it came from,
  // so every other line of assistant text can be told apart from the answer.
  const nextTurn = transcript.turns[transcript.turns.indexOf(turn) + 1]
  const start = turn.prompt?.seq ?? events[0]?.seq ?? -1
  const end = nextTurn?.prompt?.seq ?? turn.completed?.seq ?? Infinity
  const finalEvent = [...transcript.events]
    .reverse()
    .find(
      (event) =>
        event.seq >= start &&
        event.seq < end &&
        event.payload.type === "assistant_text" &&
        event.payload.text === turn.finalText,
    )
  const tools: AgentToolView[] = events.flatMap(({ payload, raw }) => {
    if (payload.type !== "tool_call_started") return []
    const result = transcript.resultByCallId.get(payload.callId)
    return [
      {
        callId: payload.callId,
        title: payload.title,
        status: result
          ? result.isError
            ? "failed"
            : "completed"
          : rawText(raw, "executionStatus") === "cancelled"
            ? "stopped"
            : rawText(raw, "status") === "pending" || rawText(raw, "status") === "running"
              ? rawText(raw, "status")
              : transcript.abandonedCallIds.has(payload.callId)
                ? "stopped"
                : rawText(raw, "status") || "running",
        input:
          rawText(raw, "input") ||
          (typeof payload.input === "string"
            ? payload.input
            : payload.input === null
              ? ""
              : JSON.stringify(payload.input, null, 2)),
        details: result?.text ?? rawText(raw, "details"),
      },
    ]
  })
  /**
   * A turn is two things in the transcript: what it worked through, and what
   * it came back with. The working — every thought, every tool call, every
   * line the agent said to itself on the way — is one collapsed segment. Only
   * the answer is left outside, because the answer is what was asked for.
   */
  const content: {
    key: string
    text?: string
    thought?: string
    tools?: AgentToolView[]
  }[] = []
  const work: { key: string; thought?: string; tools?: AgentToolView[] } = {
    key: `${turn.key}:work`,
  }
  const note = (text: string) => {
    work.thought = work.thought ? `${work.thought}\n\n${text}` : text
  }
  for (const event of events) {
    const payload = event.payload
    // The final answer is the only text that stays in the conversation.
    if (
      payload.type === "assistant_text" &&
      event.id !== finalEvent?.id &&
      payload.text.trim()
    )
      note(payload.text)
    // A thought of nothing but whitespace is a disclosure over nothing.
    if (payload.type === "reasoning" && payload.text.trim()) note(payload.text)
    if (payload.type === "tool_call_started") {
      const tool = tools.find((tool) => tool.callId === payload.callId)
      if (tool) work.tools = [...(work.tools ?? []), tool]
    }
  }
  if (work.thought || work.tools) content.push(work)
  if (finalEvent && finalEvent.payload.type === "assistant_text")
    content.push({ key: finalEvent.id, text: finalEvent.payload.text })
  if (turn.finalText !== null && !finalEvent)
    content.push({ key: `${turn.key}:answer`, text: turn.finalText })
  const completed = turn.completed?.payload
  return {
    content,
    key: turn.key,
    promptId: turn.prompt?.id,
    text: turn.finalText ?? "",
    thought: events
      .flatMap(({ payload }) => (payload.type === "reasoning" ? [payload.text] : []))
      .join("\n"),
    status:
      completed?.type === "turn_completed"
        ? (completed.terminalReason ?? completed.status)
        : "running",
    tools,
  }
}
