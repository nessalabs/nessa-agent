/**
 * The Messages tab: the list of every conversation, as a tab of its own in the
 * strip beside them.
 *
 * It belongs to the panel rather than to a conversation, the way the update tab
 * does. The list is the conversation vertical's; that it is a tab — opened from
 * the strip, closed like any other, never holding a composer — is panel chrome.
 * Choosing a row switches to that conversation's own tab, so a thread is only
 * ever drawn in one place, and the Messages tab stays in the strip to come back
 * to.
 *
 * ```text
 *   closed ──show──▶ viewing ──leave──▶ open
 *      ▲               ▲  │                │
 *      │               └──┼─────show───────┤
 *      └──────close───────┴───────close────┘
 * ```
 */

/**
 * The Messages tab's id in the tab strip. Not a conversation id: conversations
 * are identified by the ids the gateway hands out, and nothing generates this
 * one.
 */
export const MESSAGES_TAB_ID = "nessa:messages"

/** Whether the tab is in the strip, and whether it is the one on screen. */
export type MessagesTabState = "closed" | "open" | "viewing"

/**
 * The strip's ids in the order a person sees them, which is the order the
 * keyboard moves through: the Messages tab first, as the list the others were
 * opened from, then the conversations, then the update.
 */
export function stripOrder(
  messages: MessagesTabState,
  conversations: readonly string[],
  update: string | null,
): string[] {
  return [
    ...(messages === "closed" ? [] : [MESSAGES_TAB_ID]),
    ...conversations,
    ...(update === null ? [] : [update]),
  ]
}

/**
 * What happens to the Messages tab, whatever caused it: somebody chose it
 * (`show`), chose anything else in the strip (`leave`), or closed it (`close`).
 * Leaving keeps it in the strip to come back to; leaving a closed one keeps it
 * closed.
 */
export function messagesAfter(
  messages: MessagesTabState,
  event: "show" | "leave" | "close",
): MessagesTabState {
  switch (event) {
    case "show":
      return "viewing"
    case "leave":
      return messages === "viewing" ? "open" : messages
    case "close":
      return "closed"
  }
}

/**
 * Which tab closing the active one closes: the update tab, the Messages tab,
 * or the conversation — whichever is on screen.
 */
export function tabClosed(
  updateViewing: boolean,
  messages: MessagesTabState,
): "update" | "messages" | "conversation" {
  if (updateViewing) return "update"
  if (messages === "viewing") return "messages"
  return "conversation"
}

/** Whether the composer is hidden: nothing on screen is a conversation to write in. */
export function composerHidden(
  updateViewing: boolean,
  messages: MessagesTabState,
): boolean {
  return updateViewing || messages === "viewing"
}

/**
 * Whether a drop into `target`'s draft shows that conversation: only when it
 * is the active one and the Messages list is over it. A draft the composer is
 * hidden under would change with nothing on screen to say so; a drop answered
 * late, into a draft that is no longer the active one, leaves the screen alone.
 */
export function dropShowsDraft(
  messages: MessagesTabState,
  target: string,
  activeId: string,
): boolean {
  return messages === "viewing" && target === activeId
}
