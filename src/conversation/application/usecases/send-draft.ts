import type { BusyConversation, IdleConversation, UserTurn } from "../../model"
import {
  findConversation,
  replaceConversation,
  takeTurnId,
} from "../internal/ids"
import type { LocalTabs } from "../local-tabs"

/**
 * Append a sending user turn from `text` and enter thinking.
 * Returns unchanged tabs when there is nothing to send.
 */
export function beginSend(
  tabs: LocalTabs,
  input: { text: string; conversationId?: string },
): LocalTabs {
  const id = input.conversationId ?? tabs.activeId
  const conv = findConversation(tabs, id)
  if (!conv || conv.phase !== "idle") return tabs

  const text = input.text.trim()
  if (!text) return tabs

  const taken = takeTurnId(tabs)
  const userTurn: UserTurn = {
    id: taken.id,
    from: "user",
    text,
    receipt: "sending",
  }

  const next: BusyConversation = {
    id: conv.id,
    title: conv.turns.length === 0 ? text.slice(0, 48) : conv.title,
    turns: [...conv.turns, userTurn],
    draft: "",
    phase: "thinking",
    pending: "",
  }
  return replaceConversation(taken.tabs, next)
}

/** Mark the in-flight user turn delivered and append the echoed assistant reply. */
export function completeEcho(
  tabs: LocalTabs,
  conversationId: string,
  echoText: string,
): LocalTabs {
  const conv = findConversation(tabs, conversationId)
  if (!conv) return tabs

  const turns = conv.turns.map((turn) => {
    if (turn.from === "user" && turn.receipt === "sending") {
      return { ...turn, receipt: "delivered" as const }
    }
    return turn
  })

  const taken = takeTurnId(tabs)
  const next: IdleConversation = {
    id: conv.id,
    title: conv.title,
    turns: [
      ...turns,
      { id: taken.id, from: "assistant", text: echoText },
    ],
    draft: conv.draft,
    phase: "idle",
  }
  return replaceConversation(taken.tabs, next)
}

/** Leave the user turn in place and return to idle when the echo RPC fails. */
export function failSend(tabs: LocalTabs, conversationId: string): LocalTabs {
  const conv = findConversation(tabs, conversationId)
  if (!conv) return tabs

  const turns = conv.turns.map((turn) => {
    if (turn.from === "user" && turn.receipt === "sending") {
      return { ...turn, receipt: "delivered" as const }
    }
    return turn
  })

  const next: IdleConversation = {
    id: conv.id,
    title: conv.title,
    turns,
    draft: conv.draft,
    phase: "idle",
  }
  return replaceConversation(tabs, next)
}
