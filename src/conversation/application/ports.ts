import { type FileAttachment, type MessageContent } from "../model"
import type { LocalTabs } from "./local-tabs"

/**
 * What the panel may ask the product to do.
 *
 * Send is driven by an async thunk that calls `conversation.echo` via
 * `NessaClient`; these methods stay sync for UI-session tab ops.
 */
export interface ConversationGateway {
  attachFiles(tabs: LocalTabs, files: FileAttachment[], conversationId: string): LocalTabs
  removeFile(tabs: LocalTabs, id: string): LocalTabs
  stopGenerating(tabs: LocalTabs, conversationId?: string): LocalTabs
  openConversation(tabs: LocalTabs): LocalTabs
  closeConversation(tabs: LocalTabs, conversationId: string): LocalTabs
  setDraft(tabs: LocalTabs, input: { draft: MessageContent; id?: string }): LocalTabs
  moveActive(tabs: LocalTabs, direction: -1 | 1): LocalTabs
  setActive(tabs: LocalTabs, conversationId: string): LocalTabs
}

/** External effects consumed by conversation commands. */
export interface ConversationEffects {
  echo(text: string): Promise<{ text: string }>
}
