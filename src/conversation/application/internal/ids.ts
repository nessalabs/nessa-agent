import type { Conversation } from "../../model"
import type { LocalTabs } from "../local-tabs"

export function takeConversationId(tabs: LocalTabs): {
  tabs: LocalTabs
  id: string
} {
  return {
    tabs: { ...tabs, nextConversationId: tabs.nextConversationId + 1 },
    id: `c${tabs.nextConversationId}`,
  }
}

export function takeTurnId(tabs: LocalTabs): {
  tabs: LocalTabs
  id: string
} {
  return {
    tabs: { ...tabs, nextTurnId: tabs.nextTurnId + 1 },
    id: `t${tabs.nextTurnId}`,
  }
}

export function replaceConversation(tabs: LocalTabs, next: Conversation): LocalTabs {
  return {
    ...tabs,
    conversations: tabs.conversations.map((item) => (item.id === next.id ? next : item)),
  }
}

export function findConversation(tabs: LocalTabs, id: string): Conversation | undefined {
  return tabs.conversations.find((item) => item.id === id)
}
