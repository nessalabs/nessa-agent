/** Voice chat: a realtime call from a conversation's composer. See `docs/design/voice-chat.md`. */
export {
  VoiceController,
  type VoiceSnapshot,
  type VoicePhase,
} from "./application/voice-controller"
export type {
  RealtimeTransports,
  VoiceConversations,
  VoiceGateway,
} from "./application/ports"
export {
  voiceGateway,
  voiceKeyGateway,
  type VoiceClient,
  type VoiceKeyGateway,
} from "./adapters/voice-gateway"
export { webrtcTransport } from "./adapters/webrtc-transport"
export { scriptedTransport } from "./adapters/scripted-transport"
export { storeConversations } from "./adapters/store-chats"
export { VoiceProvider } from "./ui/voice-context"
export { VoiceDock } from "./ui/voice-dock"
export { useComposerVoice } from "./ui/composer-voice"
