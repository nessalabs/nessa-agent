import type { LocalStrip } from "./local-strip"

/**
 * What the panel may ask the product to do.
 *
 * Session commands are local until a remote gateway owns them.
 * `sendDraft` / `stopGenerating` are no-ops until chat RPCs exist.
 */
export interface ConversationGateway {
  sendDraft(strip: LocalStrip, conversationId?: string): LocalStrip
  stopGenerating(strip: LocalStrip, conversationId?: string): LocalStrip
  openConversation(strip: LocalStrip): LocalStrip
  closeConversation(strip: LocalStrip, conversationId: string): LocalStrip
  setDraft(strip: LocalStrip, input: { draft: string; id?: string }): LocalStrip
  setActive(strip: LocalStrip, conversationId: string): LocalStrip
}
