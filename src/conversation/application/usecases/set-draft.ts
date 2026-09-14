import { type MessageContent } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { findConversation, replaceConversation, withDraft } from "../internal"

export function setDraft(
  tabs: LocalTabs,
  input: { draft: MessageContent; id?: string },
): LocalTabs {
  const current = findConversation(tabs, input.id ?? tabs.activeId)
  if (!current) return tabs
  return replaceConversation(tabs, withDraft(current, input.draft))
}
