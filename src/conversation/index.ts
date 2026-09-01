export { ConversationClocks } from "./adapters/clock/conversation-clocks"
export type { ConversationGateway } from "./application/ports"
export {
  AGENT_HUES,
  AGENT_ICON_TONE,
  AGENT_SEED,
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
