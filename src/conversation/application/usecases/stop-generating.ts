import type { LocalTabs } from "../local-tabs"

/** No-op until a remote gateway owns turns. */
export function stopGenerating(tabs: LocalTabs, _conversationId?: string): LocalTabs {
  return tabs
}
