import type { ConversationGateway } from "../../application/ports"
import {
  closeConversation,
  openConversation,
  sendDraft,
  setActive,
  setDraft,
  stopGenerating,
} from "../../application/usecases"

/** In-process UI-session gateway. Chat send/stop are no-ops until S2 RPCs. */
export const localConversationGateway: ConversationGateway = {
  sendDraft,
  stopGenerating,
  openConversation,
  closeConversation,
  setDraft,
  setActive,
}
