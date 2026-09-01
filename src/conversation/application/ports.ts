import type { LocalStrip } from "./local-strip"

export interface ConversationGateway {
  sendDraft(strip: LocalStrip, conversationId?: string): LocalStrip
  advanceReply(strip: LocalStrip, conversationId: string): LocalStrip
  stopGenerating(strip: LocalStrip, conversationId?: string): LocalStrip
  openConversation(strip: LocalStrip): LocalStrip
  closeConversation(strip: LocalStrip, conversationId: string): LocalStrip
  setDraft(strip: LocalStrip, input: { draft: string; id?: string }): LocalStrip
  setActive(strip: LocalStrip, conversationId: string): LocalStrip
}

export interface ReplySource {
  reply(prompt: string): string
}
