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
export type AgentTurnActivityItem =
  | { key: string; kind: "thought"; text: string }
  | { key: string; kind: "tool"; tool: AgentToolView }
export type AgentTurnContentView =
  | { key: string; text: string }
  | { key: string; activity: AgentTurnActivityItem[]; running: boolean }
function rawText(raw: JsonValue, key: string): string {
  if (raw === null || Array.isArray(raw) || typeof raw !== "object") return ""
  if (!Object.hasOwn(raw, key)) return ""
  return typeof raw[key] === "string" ? raw[key] : ""
}
type ExecutionMetadata = {
  sourceTurnId: string
  executionId: string | null
  executionStatus: string | null
  activityRunning: boolean
}
function nullableRawText(raw: Record<string, JsonValue>, key: string): string | null {
  const value = raw[key]
  if (value === null || typeof value === "string") return value
  throw new Error("Transcript event has invalid source execution metadata")
}
function executionMetadata(events: readonly AgentEvent[]): ExecutionMetadata | undefined {
  const productEvents = events.filter(({ payload }) =>
    ["assistant_text", "reasoning", "tool_call_started", "turn_completed"].includes(
      payload.type,
    ),
  )
  if (productEvents.length === 0) return undefined
  const facts = productEvents.map(({ raw }) => {
    if (raw === null || Array.isArray(raw) || typeof raw !== "object")
      throw new Error("Transcript event is missing source execution metadata")
    const sourceTurnId = rawText(raw, "sourceTurnId")
    const activityRunning = raw.activityRunning
    if (!sourceTurnId || typeof activityRunning !== "boolean")
      throw new Error("Transcript event is missing source execution metadata")
    return {
      sourceTurnId,
      executionId: nullableRawText(raw, "executionId"),
      executionStatus: nullableRawText(raw, "executionStatus"),
      activityRunning,
    }
  })
  const first = facts[0]!
  if (
    facts.some(
      (fact) =>
        fact.sourceTurnId !== first.sourceTurnId ||
        fact.executionId !== first.executionId ||
        fact.executionStatus !== first.executionStatus ||
        fact.activityRunning !== first.activityRunning,
    )
  )
    throw new Error("Transcript row combines different execution metadata")
  return first
}
export function agentTurnView(turn: Turn, transcript: Transcript) {
  const events: AgentEvent[] = turn.work.flatMap((item) =>
    isToolGroup(item) ? [...item.calls] : [item],
  )
  // The builder separates finalText from work. Restore that event at its
  // original position so a pre-tool message does not move below the tools.
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
  if (finalEvent && !events.some((event) => event.id === finalEvent.id)) {
    events.push(finalEvent)
    events.sort((a, b) => a.seq - b.seq)
  }
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
  const content: AgentTurnContentView[] = []
  let activity: Extract<
    AgentTurnContentView,
    { activity: AgentTurnActivityItem[] }
  > | null = null
  let pendingThought: { key: string; text: string } | null = null
  const completed = turn.completed?.payload
  const execution = executionMetadata([
    ...events,
    ...(turn.completed ? [turn.completed] : []),
  ])
  const activityFor = (key: string) => {
    if (!activity) {
      if (!execution) throw new Error("Turn activity has no execution metadata")
      activity = {
        key,
        activity: [],
        running: execution.activityRunning,
      }
      content.push(activity)
    }
    return activity
  }
  const flushThought = () => {
    if (pendingThought?.text.trim()) {
      activityFor(pendingThought.key).activity.push({
        key: pendingThought.key,
        kind: "thought",
        text: pendingThought.text,
      })
    }
    pendingThought = null
  }
  for (const event of events) {
    const payload = event.payload
    if (payload.type === "assistant_text") {
      flushThought()
      content.push({ key: event.id, text: payload.text })
    }
    if (payload.type === "reasoning") {
      if (pendingThought) pendingThought.text += payload.text
      else pendingThought = { key: event.id, text: payload.text }
    }
    if (payload.type === "tool_call_started") {
      flushThought()
      const tool = tools.find((tool) => tool.callId === payload.callId)
      if (tool) {
        activityFor(event.id).activity.push({ key: event.id, kind: "tool", tool })
      }
    }
  }
  flushThought()
  if (turn.finalText !== null && !finalEvent)
    content.push({ key: `${turn.key}:answer`, text: turn.finalText })
  return {
    content,
    key: turn.key,
    promptId: turn.prompt?.id,
    text: turn.finalText ?? "",
    status:
      completed?.type === "turn_completed"
        ? (completed.terminalReason ?? completed.status)
        : "running",
  }
}
