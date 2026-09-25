import { conversation } from "../model"
import type { LocalTabs } from "./local-tabs"

export type SavedConversationTab = { conversationId: string; title?: string }
export type SavedConversationTabs = {
  tabs: SavedConversationTab[]
  activeConversationId?: string
}

const conversationIdPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/

function validId(value: unknown): value is string {
  return typeof value === "string" && conversationIdPattern.test(value)
}

/** Validate untrusted storage while retaining only bounded server references and titles. */
export function parseConversationTabSnapshot(
  value: unknown,
): SavedConversationTabs | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null
  const input = value as Record<string, unknown>
  if (!Array.isArray(input.tabs)) return null
  const seen = new Set<string>()
  const tabs: SavedConversationTab[] = []
  for (const raw of input.tabs) {
    if (tabs.length === 64) break
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) continue
    const item = raw as Record<string, unknown>
    if (!validId(item.conversationId) || seen.has(item.conversationId)) continue
    seen.add(item.conversationId)
    const title = typeof item.title === "string" ? item.title.trim().slice(0, 120) : ""
    tabs.push({ conversationId: item.conversationId, ...(title ? { title } : {}) })
  }
  if (!tabs.length) return null
  const active = validId(input.activeConversationId)
    ? input.activeConversationId
    : undefined
  return {
    tabs,
    ...(active && seen.has(active) ? { activeConversationId: active } : {}),
  }
}

/**
 * Attaching an image creates a gateway conversation before anything is said in
 * it. One that a view has shown to hold nothing, with nothing drafted here, is
 * not a conversation worth coming back to: saved, it would return after a reload
 * as an empty tab nobody opened. A tab whose view has not arrived is kept — it
 * may be a restored conversation that has simply not been read yet.
 */
function knownEmpty(item: LocalTabs["conversations"][number]): boolean {
  return (
    item.remote !== undefined &&
    !item.remote.truncated &&
    item.turns.length === 0 &&
    item.draft.length === 0
  )
}

export function conversationTabSnapshot(tabs: LocalTabs): SavedConversationTabs {
  // A tab onto a conversation that was deleted is kept while this window is
  // open, so a draft in it can be copied; drafts are not saved, so after a
  // reload it would bring back nothing but the notice.
  const saved = tabs.conversations.slice(0, 64).flatMap((item) =>
    item.serverConversationId && !knownEmpty(item) && item.readError !== "deleted"
      ? [
          {
            conversationId: item.serverConversationId,
            ...(item.titleEdited ? { title: item.title } : {}),
          },
        ]
      : [],
  )
  const active = tabs.conversations.find(
    (item) => item.id === tabs.activeId,
  )?.serverConversationId
  return {
    tabs: saved,
    ...(active && saved.some((item) => item.conversationId === active)
      ? { activeConversationId: active }
      : {}),
  }
}

export function restoreConversationTabs(input: SavedConversationTabs): LocalTabs {
  const conversations = input.tabs.map((item, index) => ({
    ...conversation(`c${index}`),
    ...(item.title ? { title: item.title, titleEdited: true as const } : {}),
    serverConversationId: item.conversationId,
    serverReady: true,
  }))
  if (!conversations.length) {
    return {
      conversations: [conversation("c0")],
      activeId: "c0",
      nextConversationId: 1,
      nextTurnId: 1,
    }
  }
  const activeIndex = input.tabs.findIndex(
    (item) => item.conversationId === input.activeConversationId,
  )
  return {
    conversations,
    activeId: conversations[Math.max(activeIndex, 0)].id,
    nextConversationId: conversations.length,
    nextTurnId: 1,
  }
}
