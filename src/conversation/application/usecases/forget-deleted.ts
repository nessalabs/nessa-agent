import type { LocalTabs } from "../local-tabs"
import { closeConversation } from "./close-conversation"

/**
 * Let go of every tab bound to a conversation that was deleted. Its identity is
 * refused from now on, so there is nothing a tab onto it could show or send.
 * Closed as a tab closes, so the strip is never left empty.
 */
export function forgetDeleted(tabs: LocalTabs, serverConversationId: string): LocalTabs {
  return tabs.conversations
    .filter((item) => item.serverConversationId === serverConversationId)
    .reduce((next, item) => closeConversation(next, item.id), tabs)
}
