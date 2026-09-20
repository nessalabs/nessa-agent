import type { ConversationView, Submission, SubmissionReceipt } from "./view"
import {
  type CommandFailure,
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
  forgetStoredUploads(tabs: LocalTabs, conversationId: string): LocalTabs
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
    /** Aborted when the file stops being wanted; the upload stops with it. */
    signal: AbortSignal,
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

/**
 * The gateway answered a conversation's creation, a send, or a steer with a
 * refusal it decides before admission. Unlike a lost acknowledgement, this
 * proves the message was not taken: the draft can come back, and nothing is
 * left to retry as-is. `attachment-not-found` and `attachment-unavailable` mean
 * a named image is no longer there to be sent: its reference is dead and its
 * bytes must go up again.
 */
export class SubmissionRefusedError extends Error {
  constructor(
    readonly reason: CommandFailure,
    cause?: unknown,
  ) {
    super(`The gateway refused this message (${reason}).`, { cause })
    this.name = "SubmissionRefusedError"
  }
}

/**
 * The gateway answered a conversation control with a reason of its own.
 *
 * Deliberately not a refusal: `attachment-cleanup-unavailable` is a close that
 * did happen and whose release of the conversation's uploads did not. So this
 * carries only what went wrong, never whether the control was applied — the
 * store reads the conversation again either way, as it does for any control
 * whose acknowledgement it cannot trust.
 */
export class ControlFailedError extends Error {
  constructor(
    readonly reason: CommandFailure,
    cause?: unknown,
  ) {
    super(`The gateway could not complete this control (${reason}).`, { cause })
    this.name = "ControlFailedError"
  }
}

/** The bytes being uploaded: their SHA-256, declared media type, and length. */
export type UploadedFile = { digest: string; mimeType: string; size: number }

/**
 * The gateway did not take an image's bytes, and why, as something to branch on
 * — never by reading the message. The reasons are {@link UploadFailure}, minus
 * `unreadable`, which is this window failing before the gateway saw anything.
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
