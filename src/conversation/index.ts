/**
 * Floating-panel conversation projection.
 *
 * application/view + usecases -> pure local submission and server-view mapping;
 *   queries/roster -> the Messages list joined to the open tabs
 * adapters/store -> supervised UI commands and stale-read fences; the gateway's
 *                   conversation list, paired catalogue follows, archive and delete (history.ts)
 * adapters/gateway -> injected NessaClient operations, attachment staging,
 *                     conversation and catalogue follows
 * ui -> transcript with sent-image tiles, exact permission choices, queue/stop/retry controls;
 *       the Messages list of every conversation written in; files the agent
 *       published, and referenced images, read back verified (held-files.tsx)
 *
 * A message is text plus image references. Bytes are staged at attach time and
 * a draft file carries its own upload state; `sendDraft` is the one place that
 * declines a draft, always with a reason.
 *
 * The gateway Agent owns work. Local tabs only retain drafts, server identity,
 * and displayed receipts; closing a tab never stops another surface's Agent.
 * application/saved-tabs restores remote references under fresh local names from
 * the existing allocator, fencing retired callbacks (restored-lifetimes tests).
 * attachFiles owns placement identity; resources.ts exposes immutable resource values.
 * queries/attachment-resources owns shared original retention classification
 * and resource-wide preview eviction.
 */
export { AGENT_HUES, questionAnswerReady, questionChoices } from "./model"
export { ConversationNotification } from "./ui/conversation-notification"
export { ConversationQueue } from "./ui/conversation-queue"
export { ConversationList } from "./ui/conversation-list"
export { commandErrorCleared } from "./adapters/store/history"
export { Transcript } from "./ui/transcript"
// Files a conversation holds, read back verified and shown: the panel's
// transcripts and the desktop window's alike.
export { HeldFilesProvider } from "./ui/held-files"
// Input reference tiles, also used by desktop metadata placeholders.
export { MessageImages } from "./ui/message-images"
export { PublishedFiles } from "./ui/published-files"
export { heldFileReader } from "./adapters/gateway/held-files"
export { reviewInputs } from "./adapters/gateway/review-inputs"
export { shownNote } from "./adapters/gateway/notes"
export { monotonicWallClock } from "./adapters/browser/wall-clock"
export {
  HeldFileReadError,
  type HeldFileReader,
  type HeldFileSaver,
  type UploadChange,
} from "./application/ports"
// Notes left in a conversation, placed and shown: the panel's transcripts and
// the desktop window's alike.
export { TranscriptNote } from "./ui/transcript-note"
export {
  UnreadableOverflowLine,
  UnreadablePartLine,
  UNREADABLE_MORE_SENTENCE,
  UNREADABLE_SENTENCE,
} from "./ui/unreadable-part"
export {
  referencedContent,
  historyExtent,
  historyNotices,
  placeNotes,
  placePublished,
  type HistoryExtent,
  type Note,
  type HeldFile,
  type HeldFileFailure,
  type PublishedFile,
} from "./model"
export { useConversation } from "./ui/use-conversation"

export { fromEditor, toEditor, pastedTextLabel } from "./ui/composer-content"

export {
  type FileAttachment,
  AttachmentResource,
  type AttachmentResourceData,
  type ImageReference,
  type ImageReferencePart,
  type LinkedFileReferencePart,
  type UploadFailure,
  MAX_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENT_BYTES,
  MAX_DRAFT_ATTACHMENTS,
  declaredMediaType,
  isImageFile,
  linkablePath,
  linkedFile,
  linksFilesHere,
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
// The approval modes' words, shared by the details sheet and the composer tray.
export type { ApprovalMode, ApprovalModeChoice, ConversationSelection } from "./model"
export { restoreConversations } from "./adapters/store/slice"
export {
  conversationTabSnapshot,
  parseConversationTabSnapshot,
  restoreConversationTabs,
  type SavedConversationTab,
  type SavedConversationTabs,
} from "./application/saved-tabs"

export {
  attachmentResourceUsage,
  type AttachmentResourceUsage,
} from "./application/queries/attachment-resources"

export { ClipboardProvider, SendFailure } from "./ui/send-failure"
