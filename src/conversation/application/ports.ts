import type { ConversationView, Submission, SubmissionReceipt } from "./view"
import {
  type FileAttachment,
  type ImageReference,
  type MessageContent,
  type UploadFailure,
} from "../model"
import type { LocalTabs } from "./local-tabs"

/** Pure local draft and tab operations. Remote work uses injected effects below. */
export interface ConversationGateway {
  attachFiles(tabs: LocalTabs, files: FileAttachment[], conversationId: string): LocalTabs
  removeFile(tabs: LocalTabs, id: string): LocalTabs
  changeUpload(tabs: LocalTabs, change: UploadChange): LocalTabs
  openConversation(tabs: LocalTabs): LocalTabs
  closeConversation(tabs: LocalTabs, conversationId: string): LocalTabs
  setDraft(tabs: LocalTabs, input: { draft: MessageContent; id?: string }): LocalTabs
  moveActive(tabs: LocalTabs, direction: -1 | 1): LocalTabs
  setActive(tabs: LocalTabs, conversationId: string): LocalTabs
}

/** External effects consumed by conversation commands. */
export interface ConversationEffects {
  create(conversationId: string): Promise<{ conversationId: string }>
  read(conversationId: string): Promise<ConversationView>
  send(input: Submission): Promise<SubmissionReceipt>
  steer(input: Submission): Promise<SubmissionReceipt>
  /**
   * Put an image's original bytes on the gateway and learn what it stored them
   * as. The conversation must exist (`create` first). `file` describes exactly
   * the bytes sent; the answer is the gateway's own reference — converted and
   * compressed to what the selected model takes, so possibly a different
   * digest, encoding, and size — and is the only thing a message may name.
   * Rejects with {@link AttachmentStagingError}.
   */
  stageAttachment(
    conversationId: string,
    file: UploadedFile,
    bytes: Blob,
  ): Promise<ImageReference>
  reorder(
    conversationId: string,
    executionIds: readonly string[],
  ): Promise<"applied" | "unchanged" | "queue_changed" | "priority_conflict">
  remove(conversationId: string, executionId: string): Promise<void>
  answer(
    conversationId: string,
    executionId: string,
    permissionId: string,
    optionId: string,
  ): Promise<void>
  cancel(conversationId: string, executionId: string, permissionId: string): Promise<void>
  close(conversationId: string): Promise<void>
}

/** No live transport accepted this operation; unlike a lost acknowledgement, no message was submitted. */
export class ConversationUnavailableError extends Error {
  constructor() {
    super("Not connected to the gateway. Your message was not sent.")
    this.name = "ConversationUnavailableError"
  }
}

/** The bytes being uploaded: their SHA-256, declared media type, and length. */
export type UploadedFile = { digest: string; mimeType: string; size: number }

/**
 * The gateway did not take an image's bytes, and why, as something to branch on
 * — never by reading the message. `unavailable` (no connection, no answer,
 * storage down) may succeed if tried again. `unsupported-image` and `too-large`
 * are the gateway's verdict on this image: it could not read the format, or
 * could not bring it under the selected model's limits. `rejected` is any other
 * refusal of these bytes.
 */
export class AttachmentStagingError extends Error {
  constructor(
    readonly reason: Exclude<UploadFailure, "unreadable">,
    cause?: unknown,
  ) {
    super(`The gateway did not take this image (${reason}).`, { cause })
    this.name = "AttachmentStagingError"
  }
}

/** One step in a draft file's upload. `stored` carries the gateway's reference. */
export type UploadChange = { fileId: string } & (
  | { to: "uploading" }
  | { to: "stored"; image: ImageReference }
  | { to: "failed"; reason: UploadFailure }
  | { to: "not-started" }
)
