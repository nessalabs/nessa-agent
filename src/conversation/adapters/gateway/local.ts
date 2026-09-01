import type { ConversationGateway } from "../../application/ports"
import {
  advanceReply,
  closeConversation,
  openConversation,
  sendDraft,
  setActive,
  setDraft,
  stopGenerating,
} from "../../application/usecases"
import { standInReply } from "../../application/internal"

export const localConversationGateway: ConversationGateway = {
  sendDraft,
  advanceReply: (strip, conversationId) =>
    advanceReply(strip, conversationId, standInReply),
  stopGenerating,
  openConversation,
  closeConversation,
  setDraft,
  setActive,
}
