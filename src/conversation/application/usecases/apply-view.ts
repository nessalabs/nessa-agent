import {
  referencedContent,
  type Conversation,
  type Receipt,
  type Turn,
  type UserTurn,
} from "../../model"
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
      // Local content keeps its previews. Without it — another surface's turn,
      // or this one after a reload — the images are known only by reference and
      // the files only by the path the turn named.
      content:
        local?.content ??
        referencedContent(message.userText, message.attachments, message.files),
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
      content:
        local?.content ??
        referencedContent(pending.text, pending.attachments, pending.files),
      receipt: "queued",
      executionId: pending.executionId,
      actionId: local?.actionId,
      mode: local?.mode,
    })
  }
  // A view can race an admitted send. Keep local intent the server has not
  // acknowledged yet; never resend as a side effect of read. A queued receipt is
  // the server's own fact, so a complete queue that omits that identity retires
  // the classification instead of leaving it active work forever. An incomplete
  // queue proves nothing, and local failures stay visible either way.
  const confirmedQueued: Receipt[] = ["accepted", "queued"]
  for (const turn of current.turns) {
    if (
      turn.from === "user" &&
      turn.executionId &&
      turn.actionId &&
      !seen.has(turn.executionId) &&
      turn.receipt !== "delivered" &&
      !(view.queueComplete && confirmedQueued.includes(turn.receipt))
    )
      projected.push(turn)
  }
  const retainedError = current.turns.some(
    (turn) =>
      turn.from === "user" &&
      turn.executionId &&
      seen.has(turn.executionId) &&
      turn.error === current.error,
  )
    ? undefined
    : current.error
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
    error: retainedError,
    // The reason describes `error`; clearing one without the other would leave
    // a notice branching on a failure the conversation no longer reports.
    failure: retainedError === undefined ? undefined : current.failure,
    remote: {
      runtime: view.runtime,
      running: view.messages.some((message) => message.status === "running"),
      truncated: view.truncated,
      queueComplete: view.queueComplete,
      permissionViewError: view.permissionViewError,
      permissions: view.permissions,
      questions: view.questions,
      tools: view.tools,
      pending: view.pending,
      capabilities: view.capabilities,
    },
  }
}
