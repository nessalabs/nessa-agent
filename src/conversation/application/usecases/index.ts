/**
 * Conversation use cases. Each file is one command the panel can issue.
 *
 * Server-owned (will move behind the remote gateway):
 *   sendDraft, advanceReply, stopGenerating, openConversation, closeConversation
 *
 * UI session (stay on the panel until they persist):
 *   setDraft, setActive
 */
export { advanceReply } from "./advance-reply"
export { closeConversation } from "./close-conversation"
export { openConversation } from "./open-conversation"
export { sendDraft } from "./send-draft"
export { setActive } from "./set-active"
export { setDraft } from "./set-draft"
export { stopGenerating } from "./stop-generating"
