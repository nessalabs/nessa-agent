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
  /** What the call does, in the stream's vocabulary; `other` when unsaid. */
  kind: string
  status: string
  input: string
  details: string
}
/** One thing a turn did, in the order it did it. */
export type WorkStep = {
  key: string
  text?: string
  thought?: string
  tool?: AgentToolView
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
        kind: payload.kind,
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
   * line the agent said to itself on the way — is one collapsed segment, kept
   * in the order it happened so a rationale still sits beside the call it
   * explains. Only the answer is left outside, because the answer is what was
   * asked for.
   */
  const steps: WorkStep[] = []
  for (const event of events) {
    const payload = event.payload
    // The final answer is the only text that stays in the conversation.
    if (
      payload.type === "assistant_text" &&
      event.id !== finalEvent?.id &&
      payload.text.trim()
    )
      steps.push({ key: event.id, text: payload.text.trim() })
    // A thought of nothing but whitespace is a disclosure over nothing — and
    // one that was coalesced onto the next thought is leading blank lines in
    // it, which the sheet would render as a gap.
    if (payload.type === "reasoning" && payload.text.trim())
      steps.push({ key: event.id, thought: payload.text.trim() })
    if (payload.type === "tool_call_started") {
      const tool = tools.find((tool) => tool.callId === payload.callId)
      if (tool) steps.push({ key: event.id, tool })
    }
  }
  const content: { key: string; text?: string; work?: WorkStep[] }[] = []
  if (steps.length) content.push({ key: `${turn.key}:work`, work: steps })
  // Keyed on the turn, not the event: which text is the answer changes while a
  // turn runs, and a key that moved with it would remount the bubble.
  if (turn.finalText !== null)
    content.push({ key: `${turn.key}:answer`, text: turn.finalText })
  const completed = turn.completed?.payload
  return {
    content,
    key: turn.key,
    promptId: turn.prompt?.id,
    status:
      completed?.type === "turn_completed"
        ? (completed.terminalReason ?? completed.status)
        : "running",
  }
}
