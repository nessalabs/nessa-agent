/**
 * Floating-panel conversation projection.
 *
 * application/view + usecases -> pure local submission and server-view mapping;
 *   queries/roster -> the Messages list joined to the open tabs
 * adapters/store -> supervised UI commands and stale-read fences; the gateway's
 *                   conversation list, archive and delete (history.ts)
 * adapters/gateway -> injected NessaClient operations, attachment staging, and bounded polling
 * ui -> transcript with sent-image tiles, exact permission choices, queue/stop/retry controls;
 *       the Messages list of every conversation written in
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
export { ConversationList } from "./ui/conversation-list"
export { commandErrorCleared } from "./adapters/store/history"
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
  declaredMediaType,
  isImageFile,
  linkablePath,
  linkedFile,
  previewableImage,
  validDraftAttachments,
} from "./model"

// Why an upload failed, in words, and whether to offer a retry. The reasons are
// this vertical's, so the sentences for them are too; the panel says them.
export {
  uploadFailureSummary,
  uploadFailureText,
  worthRetrying,
} from "./application/usecases/upload-failure"

export { ConversationTabMenu, ConversationDetails } from "./ui/conversation-details"
export { restoreConversations } from "./adapters/store/slice"
export {
  conversationTabSnapshot,
  parseConversationTabSnapshot,
  restoreConversationTabs,
  type SavedConversationTab,
  type SavedConversationTabs,
} from "./application/saved-tabs"
