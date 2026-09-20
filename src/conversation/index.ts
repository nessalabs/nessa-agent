/**
 * Floating-panel conversation projection.
 *
 * application/view + usecases -> pure local submission and server-view mapping
 * adapters/store -> supervised UI commands and stale-read fences
 * adapters/gateway -> injected NessaClient operations, attachment staging, and bounded polling
 * ui -> transcript with sent-image tiles, exact permission choices, queue/stop/retry controls
 *
 * A message is text plus image references. Bytes are staged at attach time and
 * a draft file carries its own upload state; `sendDraft` is the one place that
 * declines a draft, always with a reason.
 *
 * The gateway Agent owns work. Local tabs only retain drafts, server identity,
 * and displayed receipts; closing a tab never stops another surface's Agent.
 */
export { AGENT_HUES } from "./model"
export { ConversationNotification } from "./ui/conversation-notification"
export { ConversationQueue } from "./ui/conversation-queue"
export { Transcript } from "./ui/transcript"
export { useConversation } from "./ui/use-conversation"

export { fromEditor, toEditor, pastedTextLabel } from "./ui/composer-content"

export {
  type FileAttachment,
  type ImageReference,
  type UploadFailure,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  isImageFile,
  validDraftAttachments,
} from "./model"

export { ConversationTabMenu, ConversationDetails } from "./ui/conversation-details"
export { restoreConversations } from "./adapters/store/slice"
export {
  conversationTabSnapshot,
  parseConversationTabSnapshot,
  restoreConversationTabs,
  type SavedConversationTab,
  type SavedConversationTabs,
} from "./application/saved-tabs"
