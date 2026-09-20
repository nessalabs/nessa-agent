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
  closeConversation,
  closeTab,
  controlConversation,
  openConversation,
  refreshConversation,
  removeFile,
  sendDraft,
  setActive,
  setDraft,
  stageAttachment,
  uploadChanged,
} from "./adapters/store/slice"
export { scenarioEffects } from "./adapters/scenario/effects"
export {
  AttachmentStagingError,
  SubmissionRefusedError,
  type ConversationEffects,
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
  isImageFile,
  previewableImage,
  validDraftAttachments,
} from "./model"
export {
  uploadFailureSummary,
  uploadFailureText,
  worthRetrying,
} from "./application/usecases/upload-failure"
