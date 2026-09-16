import { textContent, type Conversation, type Turn, type UserTurn } from "../../model"
import type { ConversationView } from "../view"

/** Replace server facts while preserving unsent drafts and unacknowledged local submissions. */
export function applyView(current: Conversation, view: ConversationView): Conversation {
  const known = new Map<string, UserTurn>()
  for (const turn of current.turns)
    if (turn.from === "user" && turn.executionId) known.set(turn.executionId, turn)
  const waiting = new Set(view.pending.map((item) => item.executionId))
  const projected: Turn[] = []
  const seen = new Set<string>()
  for (const message of view.messages) {
    seen.add(message.executionId)
    const local = known.get(message.executionId)
    projected.push({
      id: local?.id ?? `${message.executionId}:user`,
      from: "user",
      content: local?.content ?? textContent(message.userText),
      receipt: waiting.has(message.executionId)
        ? "queued"
        : message.status === "queued"
          ? "accepted"
          : "delivered",
      executionId: message.executionId,
      actionId: local?.actionId,
      mode: local?.mode,
      steeringTarget: message.steeringTarget,
      steeringOffset: message.steeringOffset,
    })
    if (
      (message.status !== "queued" &&
        view.tools.some((tool) => tool.executionId === message.executionId)) ||
      message.parts
        .filter((part) => part.kind === "text")
        .map((part) => part.text)
        .join("") ||
      message.parts
        .filter((part) => part.kind === "thought")
        .map((part) => part.text)
        .join("") ||
      message.error ||
      !["queued", "running"].includes(message.status)
    ) {
      projected.push({
        id: `${message.executionId}:assistant`,
        from: "assistant",
        parts: message.parts,
        executionId: message.executionId,
        text: message.parts
          .filter((part) => part.kind === "text")
          .map((part) => part.text)
          .join(""),
        thought: message.parts
          .filter((part) => part.kind === "thought")
          .map((part) => part.text)
          .join(""),
        status: message.error ?? message.status,
      })
    }
  }
  for (const pending of view.pending) {
    if (seen.has(pending.executionId)) continue
    seen.add(pending.executionId)
    const local = known.get(pending.executionId)
    projected.push({
      id: local?.id ?? `${pending.executionId}:user`,
      from: "user",
      content: local?.content ?? textContent(pending.text),
      receipt: "queued",
      executionId: pending.executionId,
      actionId: local?.actionId,
      mode: local?.mode,
    })
  }
  // A view can race an admitted send. Keep unknown/sending/accepted local receipts
  // until the server includes their identity; never resend as a side effect of read.
  for (const turn of current.turns) {
    if (
      turn.from === "user" &&
      turn.executionId &&
      turn.actionId &&
      !seen.has(turn.executionId) &&
      turn.receipt !== "delivered"
    )
      projected.push(turn)
  }
  const busy =
    view.pending.length > 0 ||
    view.messages.some((message) => ["queued", "running"].includes(message.status))
  return {
    ...current,
    turns: projected,
    revision: view.revision,
    readRequest: undefined,
    phase: busy
      ? view.messages.some(
          (message) =>
            message.status === "running" &&
            message.parts
              .filter((part) => part.kind === "text")
              .map((part) => part.text)
              .join(""),
        )
        ? "streaming"
        : "thinking"
      : "idle",
    pending: "",
    readError: undefined,
    error: current.turns.some(
      (turn) =>
        turn.from === "user" &&
        turn.executionId &&
        seen.has(turn.executionId) &&
        turn.error === current.error,
    )
      ? undefined
      : current.error,
    remote: {
      runtime: view.runtime,
      running: view.messages.some((message) => message.status === "running"),
      truncated: view.truncated,
      queueComplete: view.queueComplete,
      permissionViewError: view.permissionViewError,
      permissions: view.permissions,
      tools: view.tools,
      pending: view.pending,
      capabilities: view.capabilities,
    },
  }
}
