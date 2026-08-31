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

/**
 * In-process stand-in for the conversation server. Swap this module for a
 * remote adapter when the gateway exists; the panel still only paints the
 * strip it gets back.
 */
export const localConversationGateway: ConversationGateway = {
  sendDraft,
  advanceReply,
  stopGenerating,
  openConversation,
  closeConversation,
  setDraft,
  setActive,
}
