import type { ConversationGateway } from "../../application/ports"
import {
  closeConversation,
  openConversation,
  setActive,
  setDraft,
  stopGenerating,
} from "../../application/usecases"

/** In-process UI-session gateway. Send uses the remote echo thunk, not this. */
export const localConversationGateway: ConversationGateway = {
  stopGenerating,
  openConversation,
  closeConversation,
  setDraft,
  setActive,
}
