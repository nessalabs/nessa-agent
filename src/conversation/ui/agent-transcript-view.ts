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
/** One ordered transcript sibling: working, an answer, or a local Nessa notice. */
export type AgentTurnContentView = {
  key: string
  text?: string
  work?: WorkStep[]
  running?: boolean
  notice?: string
}
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
function localNotice(raw: JsonValue): { id: string; text: string } | undefined {
  if (raw === null || Array.isArray(raw) || typeof raw !== "object") return undefined
  const value = raw.localNotice
  if (value === null || Array.isArray(value) || typeof value !== "object")
    return undefined
  if (!Object.hasOwn(value, "id") || !Object.hasOwn(value, "text")) return undefined
  return typeof value.id === "string" && typeof value.text === "string"
    ? { id: value.id, text: value.text }
    : undefined
}
function executionMetadata(events: readonly AgentEvent[]): ExecutionMetadata | undefined {
  const productEvents = events.filter(
    ({ payload, raw }) =>
      ["assistant_text", "reasoning", "tool_call_started", "turn_completed"].includes(
        payload.type,
      ) ||
      (payload.type === "unknown" && localNotice(raw) !== undefined),
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
  const [first, ...rest] = facts
  if (!first) return undefined
  if (
    rest.some(
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
  // The shared builder may hold a tool run across an `unknown` event because
  // it cannot know that this product renders the event as a local notice.
  // Sequence is the source ordering contract; restore it before placing that
  // product-specific sibling so work cannot move across the notice.
  events.sort((left, right) => left.seq - right.seq)
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
  const content: AgentTurnContentView[] = []
  let steps: WorkStep[] = []
  const execution = executionMetadata([
    ...events,
    ...(turn.completed ? [turn.completed] : []),
  ])
  // A thought streams in as many events — "hello", " ", "world" — and is one
  // thought. The run is gathered and written once something else happens, so
  // a lone space joins its neighbours instead of being mistaken for nothing.
  let thinking: { key: string; text: string } | null = null
  const flushThought = () => {
    // A run of nothing but whitespace is a disclosure over nothing, and the
    // ends of a real one are blank lines the sheet would draw as a gap.
    const text = thinking?.text.trim()
    if (thinking && text) steps.push({ key: thinking.key, thought: text })
    thinking = null
  }
  const flushWork = () => {
    flushThought()
    const first = steps[0]
    if (!first) return
    if (!execution) throw new Error("Turn activity has no execution metadata")
    content.push({
      key: `${turn.key}:work:${first.key}`,
      work: steps,
      running: execution.activityRunning,
    })
    steps = []
  }
  for (const event of events) {
    const payload = event.payload
    if (payload.type === "reasoning") {
      if (thinking) thinking.text += payload.text
      else thinking = { key: event.id, text: payload.text }
      continue
    }
    if (payload.type === "assistant_text" || payload.type === "tool_call_started")
      flushThought()
    // The final answer is the only text that stays in the conversation. What
    // the agent said on the way is Markdown, where an indented code block or
    // a trailing hard break is meaning: whitespace decides whether it counts,
    // and is never cut from it. (A thought is plain text, and is trimmed.)
    if (
      payload.type === "assistant_text" &&
      event.id !== finalEvent?.id &&
      payload.text.trim()
    )
      steps.push({ key: event.id, text: payload.text })
    if (payload.type === "tool_call_started") {
      const tool = tools.find((tool) => tool.callId === payload.callId)
      if (tool) steps.push({ key: event.id, tool })
    }
    if (payload.type === "unknown") {
      const notice = localNotice(event.raw)
      if (notice) {
        flushWork()
        content.push({ key: `notice:${notice.id}`, notice: notice.text })
      }
    }
  }
  flushWork()
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
