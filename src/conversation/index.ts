/**
 * Conversation vertical. The panel paints this projection and dispatches
 * the named commands. Product rules live in `application/usecases` and are
 * reached through `adapters/gateway` — today a local stand-in, tomorrow
 * the server.
 *
 * Use cases: `application/usecases/`.
 */
export { ConversationClocks } from "./adapters/clock/conversation-clocks"
export {
  advanceReply,
  closeConversation,
  conversationReducer,
  openConversation,
  sendDraft,
  setActiveId,
  setDraft,
  stopGenerating,
} from "./adapters/store/slice"
export type { ConversationGateway } from "./application/ports"
export {
  conversation,
  emptyStrip,
  type Conversation,
  type ConversationStrip,
  type Phase,
  type Receipt,
  type Turn,
} from "./model"
export { Transcript } from "./ui/transcript"
export { useConversation } from "./ui/use-conversation"
