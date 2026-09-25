import { conversation } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { takeConversationId } from "../internal"
import { UNTITLED } from "../queries/roster"

/**
 * Show a conversation the gateway listed. A tab already holding it is the one
 * shown, so choosing a row twice never opens a second tab onto one
 * conversation. Otherwise it is reopened as a new tab bound to the gateway's
 * identity and marked ready, exactly as a tab restored after a reload is: the
 * active tab's polling then reads its history.
 *
 * The listed title names the tab — the list's name for an untitled one, so the
 * row and the tab it opens agree — and the view that arrives keeps it current
 * (see `applyView`). It is not marked as edited, because nobody here chose it.
 */
export function openListed(
  tabs: LocalTabs,
  listed: { serverConversationId: string; title: string | null },
): LocalTabs {
  const open = tabs.conversations.find(
    (item) => item.serverConversationId === listed.serverConversationId,
  )
  if (open) return { ...tabs, activeId: open.id }
  const next = takeConversationId(tabs)
  const reopened = conversation(next.id)
  return {
    ...next.tabs,
    conversations: [
      ...next.tabs.conversations,
      {
        ...reopened,
        title: listed.title ?? UNTITLED,
        serverConversationId: listed.serverConversationId,
        serverReady: true,
      },
    ],
    activeId: next.id,
  }
}
