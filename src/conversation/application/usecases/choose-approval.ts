import { modelChoiceOpen, type ApprovalMode } from "../../model"
import type { LocalTabs } from "../local-tabs"

/**
 * Choose the approval mode a conversation is created with. Only a tab whose
 * conversation does not exist yet takes one: changing an existing
 * conversation's mode is the gateway's to do (ADR 231 §4), so it is left alone.
 */
export function chooseApproval(
  tabs: LocalTabs,
  input: { id: string; mode: ApprovalMode },
): LocalTabs {
  const current = tabs.conversations.find((item) => item.id === input.id)
  if (!current || !modelChoiceOpen(current)) return tabs
  return {
    ...tabs,
    conversations: tabs.conversations.map((item) =>
      item.id === input.id ? { ...item, approvalChoice: input.mode } : item,
    ),
  }
}
