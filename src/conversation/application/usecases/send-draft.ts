import {
  contentText,
  hasFileAttachments,
  type MessageContent,
  type UserTurn,
} from "../../model"
import type { ConversationErrorCode } from "@nessa/client"
import { findConversation, replaceConversation, takeTurnId } from "../internal/ids"
import type { LocalTabs } from "../local-tabs"

/** Retain each local submission independently; busy work does not reject a queueable draft. */
export function beginSend(
  tabs: LocalTabs,
  input: {
    content: MessageContent
    conversationId: string
    executionId: string
    actionId: string
    mode: "queued" | "steering"
  },
): LocalTabs {
  const conv = findConversation(tabs, input.conversationId)
  if (!conv || hasFileAttachments(input.content) || hasFileAttachments(conv.draft))
    return tabs
  const text = contentText(input.content)
  if (!text.trim()) return tabs
  const taken = takeTurnId(tabs)
  const userTurn: UserTurn = {
    id: taken.id,
    from: "user",
    content: input.content,
    receipt: "sending",
    executionId: input.executionId,
    actionId: input.actionId,
    mode: input.mode,
  }
  return replaceConversation(taken.tabs, {
    ...conv,
    cancellationStatus: undefined,
    title:
      conv.turns.length === 0 && !conv.titleEdited
        ? text.trim().slice(0, 48)
        : conv.title,
    turns: [...conv.turns, userTurn],
    draft: [],
    phase: "thinking",
    pending: "",
    error: undefined,
    errorCode: undefined,
  })
}

/** A transport failure is not proof that admission failed; retain IDs for explicit retry. */
export function failSend(
  tabs: LocalTabs,
  conversationId: string,
  executionId: string,
  detail: string,
  uncertain = true,
  errorCode?: ConversationErrorCode,
): LocalTabs {
  const conv = findConversation(tabs, conversationId)
  if (!conv) return tabs
  const failedTurn = conv.turns.find(
    (turn) =>
      turn.from === "user" &&
      turn.executionId === executionId &&
      turn.receipt === "sending",
  )
  const recoveredDraft =
    !uncertain && conv.draft.length === 0 && failedTurn?.from === "user"
      ? failedTurn.content
      : conv.draft
  const otherWork =
    conv.remote?.running ||
    !!conv.remote?.pending.length ||
    conv.turns.some(
      (turn) =>
        turn.from === "user" &&
        turn.executionId !== executionId &&
        ["sending", "accepted", "queued", "unknown"].includes(turn.receipt),
    )
  return replaceConversation(tabs, {
    ...(uncertain || otherWork ? conv : { ...conv, phase: "idle" as const }),
    error: detail,
    errorCode,
    draft: recoveredDraft,
    draftReset:
      recoveredDraft === conv.draft ? conv.draftReset : (conv.draftReset ?? 0) + 1,
    turns: conv.turns.map((turn) =>
      turn.from === "user" &&
      turn.executionId === executionId &&
      turn.receipt === "sending"
        ? { ...turn, receipt: uncertain ? "unknown" : "failed", error: detail }
        : turn,
    ),
  })
}
