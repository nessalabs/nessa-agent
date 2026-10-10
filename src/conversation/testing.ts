/**
 * What another context's tests may take from the conversation vertical.
 *
 * The barrel (`index.ts`) is what product code imports, and it also exports
 * components, which drag the whole design system into a test that wanted one
 * predicate. The alternatives are worse: reaching past the barrel into
 * `adapters/store/slice` and `application/ports`, or mocking it with a copy of
 * a rule — a copy that keeps passing after the rule changes.
 *
 * This is the door instead: the store's commands, the scenario substitute, the
 * typed errors, the hook, and the pure parts of the barrel, with no component
 * among them. A test that needs the barrel mocked mocks it with this module, so
 * there is one definition of everything it uses. Product code does not import
 * this file.
 */
export {
  attachFiles,
  bindConversation,
  closeConversation,
  closeTab,
  controlConversation,
  openConversation,
  readStarted,
  followConversation,
  unfollowConversation,
  removeFile,
  sendDraft,
  setActive,
  setDraft,
  stageAttachment,
  uploadChanged,
  viewReceived,
} from "./adapters/store/slice"
export { scenarioEffects } from "./adapters/scenario/effects"
import {
  ConversationReadFailedError,
  type ConversationEffects,
} from "./application/ports"
import type { ConversationView } from "./application/view"

export {
  AttachmentStagingError,
  ConversationReadFailedError,
  SubmissionRefusedError,
  type ConversationEffects,
  type ConversationFollower,
} from "./application/ports"
export type { ConversationView } from "./application/view"
export { useConversation } from "./ui/use-conversation"
export { fromEditor, toEditor, pastedTextLabel } from "./ui/composer-content"
export {
  type FileAttachment,
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
export {
  uploadFailureSummary,
  uploadFailureText,
  worthRetrying,
} from "./application/usecases/upload-failure"

/**
 * A `follow` that reads once per follow: each call asks `read` and tells the
 * follower its answer, or the panel's word for why there was none. What a
 * store test needs from a gateway that is not changing under it; nothing is
 * told after the follow is stopped.
 */
export function followByReading(
  read: (conversationId: string) => Promise<ConversationView>,
): ConversationEffects["follow"] {
  return (conversationId, follower) => {
    let stopped = false
    void Promise.resolve()
      .then(() => read(conversationId))
      .then(
        (view) => {
          if (!stopped) follower.view(view)
        },
        (error: unknown) => {
          if (stopped) return
          follower.failed(
            error instanceof ConversationReadFailedError ? error.reason : "unavailable",
          )
        },
      )
    return () => {
      stopped = true
    }
  }
}
