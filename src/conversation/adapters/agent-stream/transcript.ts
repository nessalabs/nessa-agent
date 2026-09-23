import type {
  AgentEvent,
  AgentEventPayload,
  JsonValue,
  ToolKind,
} from "@nessalabs/agent-stream"
import { TranscriptBuilder, type Transcript } from "@nessalabs/agent-stream/transcript"
import { contentText, type Turn } from "../../model"
import type { ConversationView } from "../../application/view"

/**
 * The provider's own word for what a call does, in the stream's vocabulary.
 * A title cannot stand in for it — "grep -l" and "Find" are both searches
 * and neither title says so — and an unsaid kind stays `other` rather than
 * being guessed at.
 */
function toolKind(kind: string): ToolKind {
  switch (kind) {
    case "read":
      return "file_read"
    case "edit":
    case "delete":
    case "move":
      return "file_edit"
    case "search":
      return "search"
    case "fetch":
      return "web"
    case "execute":
      return "shell"
    case "think":
      return "plan"
    default:
      return "other"
  }
}
/** Map a bounded replacement view, never append polling snapshots to a live log.
 * Provider offsets preserve observation order; steering offsets mark local admission.
 * Raw DTOs retain details the common contract does not model, such as partial output.
 */
export function agentTranscript(
  sessionId: string,
  turns: readonly Turn[],
  tools: ConversationView["tools"],
): Transcript {
  const events: AgentEvent[] = []
  const push = (id: string, payload: AgentEventPayload, raw: JsonValue = null) => {
    events.push({
      id,
      sessionId,
      seq: events.length,
      ts: null,
      agentPath: [],
      payload,
      raw,
    })
  }
  const answers = new Set(
    turns
      .filter((turn) => turn.from === "assistant" && turn.status !== "injected")
      .map((turn) => turn.executionId),
  )
  for (const turn of turns) {
    if (turn.from === "user") {
      if (turn.steeringTarget && answers.has(turn.steeringTarget)) continue
      if (turn.receipt !== "queued")
        push(turn.id, {
          type: "user_message",
          text: contentText(turn.content),
          synthetic: false,
        })
      continue
    }
    if (turn.status === "injected") continue
    const inputs = turns
      .flatMap((input) => {
        if (
          input.from !== "user" ||
          !input.steeringTarget ||
          input.steeringTarget !== turn.executionId
        )
          return []
        if (input.steeringOffset === undefined)
          throw new Error("Injected input has no observation offset")
        return [{ ...input, steeringOffset: input.steeringOffset }]
      })
      .sort((a, b) => a.steeringOffset - b.steeringOffset)
    let inputIndex = 0
    let buffered = ""
    let bufferKind: "text" | "thought" = "text"
    let bufferOffset = 0
    let bufferMessageId: string | undefined
    const execution = {
      sourceTurnId: turn.id,
      executionId: turn.executionId ?? null,
      executionStatus: turn.status ?? null,
      activityRunning:
        turn.status === undefined || ["queued", "running"].includes(turn.status),
    }
    const flush = () => {
      if (buffered)
        push(
          `${turn.id}:${bufferOffset}`,
          {
            type: bufferKind === "text" ? "assistant_text" : "reasoning",
            text: buffered,
            block: bufferMessageId
              ? { messageId: bufferMessageId, index: bufferOffset }
              : null,
          },
          execution,
        )
      buffered = ""
    }
    const insertInputs = (offset: number) => {
      while (inputIndex < inputs.length) {
        const input = inputs[inputIndex]
        if (!input) break
        if (input.steeringOffset > offset) break
        flush()
        push(input.id, {
          type: "user_message",
          text: contentText(input.content),
          synthetic: false,
        })
        inputIndex++
      }
    }
    const seenTools = new Set<string>()
    for (const part of turn.parts) {
      insertInputs(part.offset)
      if (part.kind === "local_notice") {
        flush()
        const noticeId = part.noticeId
        if (!noticeId) throw new Error("Local notice has no identity")
        push(
          `${turn.id}:notice:${noticeId}`,
          {
            type: "unknown",
            wireType: "nessa.local_review_declined",
            subtype: null,
          },
          {
            ...execution,
            localNotice: { id: noticeId, text: part.text },
          },
        )
        continue
      }
      if (part.kind !== "tool") {
        if (buffered && (bufferKind !== part.kind || bufferMessageId !== part.messageId))
          flush()
        if (!buffered) bufferOffset = part.offset
        bufferMessageId = part.messageId
        bufferKind = part.kind
        buffered += part.text
        continue
      }
      flush()
      const tool = tools.find(
        (tool) => tool.executionId === turn.executionId && tool.toolId === part.toolId,
      )
      if (!tool) continue
      const callId = JSON.stringify([tool.executionId, tool.toolId])
      let input: JsonValue = tool.input || null
      if (tool.input) {
        try {
          input = JSON.parse(tool.input) as JsonValue
        } catch {
          /* exact plain text */
        }
      }
      if (!seenTools.has(part.toolId))
        push(
          `${callId}:start`,
          {
            type: "tool_call_started",
            callId,
            name: tool.title,
            kind: toolKind(tool.kind),
            title: tool.title,
            input,
          },
          { ...tool, ...execution },
        )
      seenTools.add(part.toolId)
      const last = [...turn.parts]
        .reverse()
        .find((item) => item.kind === "tool" && item.toolId === part.toolId)
      if (last === part && (tool.status === "completed" || tool.status === "failed"))
        push(`${callId}:result`, {
          type: "tool_call_completed",
          callId,
          result: {
            text: tool.details,
            isError: tool.status === "failed",
            structured: null,
            images: [],
          },
        })
    }
    flush()
    insertInputs(Infinity)
    if (turn.status && !["running", "queued"].includes(turn.status)) {
      push(
        `${turn.id}:end`,
        {
          type: "turn_completed",
          status:
            turn.status === "completed"
              ? "completed"
              : turn.status === "cancelled"
                ? "interrupted"
                : "error",
          stopReason: turn.status,
          terminalReason: turn.status,
          finalText: null,
          usage: null,
          durationMs: null,
          numTurns: null,
          permissionDenials: [],
        },
        execution,
      )
    }
  }
  const builder = new TranscriptBuilder({ sessionId })
  builder.push(events)
  return builder.snapshot({ live: true })
}
