import { modelChoiceOpen, type ModelChoice } from "../../model"
import type { LocalTabs } from "../local-tabs"
import { openConversation } from "./open-conversation"

/**
 * Choose the model a conversation is created with.
 *
 * A tab whose conversation has not been created takes the choice itself. One
 * that already exists keeps its model (ADR 231 §2), so the choice opens a new
 * tab with it instead, and that tab becomes the active one.
 */
export function chooseModel(
  tabs: LocalTabs,
  input: { id: string; choice: ModelChoice },
): LocalTabs {
  const current = tabs.conversations.find((item) => item.id === input.id)
  if (!current) return tabs
  if (modelChoiceOpen(current)) {
    return {
      ...tabs,
      conversations: tabs.conversations.map((item) =>
        item.id === input.id ? { ...item, modelChoice: input.choice } : item,
      ),
    }
  }
  const opened = openConversation(tabs)
  return {
    ...opened,
    conversations: opened.conversations.map((item) =>
      item.id === opened.activeId ? { ...item, modelChoice: input.choice } : item,
    ),
  }
}
