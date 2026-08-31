import type { ConversationStrip } from "../model"

/**
 * Everything the panel may ask the product to do.
 *
 * The UI does not own these operations. Today a local adapter runs them
 * in-process. Tomorrow the same methods are the server: the panel still
 * only sends a command and paints the strip it gets back.
 */
export interface ConversationGateway {
  sendDraft(strip: ConversationStrip, conversationId?: string): ConversationStrip
  advanceReply(strip: ConversationStrip, conversationId: string): ConversationStrip
  stopGenerating(strip: ConversationStrip, conversationId?: string): ConversationStrip
  openConversation(strip: ConversationStrip): ConversationStrip
  closeConversation(strip: ConversationStrip, conversationId: string): ConversationStrip
  setDraft(
    strip: ConversationStrip,
    input: { draft: string; id?: string },
  ): ConversationStrip
  setActive(strip: ConversationStrip, conversationId: string): ConversationStrip
}

/**
 * Stands in for the agent runtime. The advance-reply use case asks this
 * for text; it does not invent a reply itself. A real runtime replaces it.
 */
export interface ReplySource {
  reply(prompt: string): string
}
