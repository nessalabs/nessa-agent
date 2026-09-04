import type { LocalTabs } from "./local-tabs"

/**
 * What the panel may ask the product to do.
 *
 * `sendDraft` / `stopGenerating` are no-ops until chat RPCs exist.
 */
export interface ConversationGateway {
  sendDraft(tabs: LocalTabs, conversationId?: string): LocalTabs
  stopGenerating(tabs: LocalTabs, conversationId?: string): LocalTabs
  openConversation(tabs: LocalTabs): LocalTabs
  closeConversation(tabs: LocalTabs, conversationId: string): LocalTabs
  setDraft(tabs: LocalTabs, input: { draft: string; id?: string }): LocalTabs
  setActive(tabs: LocalTabs, conversationId: string): LocalTabs
}
