import type { ConversationView, Submission, SubmissionReceipt } from "./view"
import {
  type CommandFailure,
  type FileAttachment,
  type ImageReference,
  type MessageContent,
  type ReadFailure,
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
  /**
   * Read the gateway's current bounded view of a conversation. Rejects with
   * {@link ConversationReadFailedError}, so no caller has to look at a wire code
   * or a sentence to find out what went wrong.
   */
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
  /** Answer one question the agent asked; null choices decline it. */
  answerQuestion(
    conversationId: string,
    executionId: string,
    questionId: string,
    choices: readonly { key: string; values: string[]; ownWords?: string }[] | null,
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
 * A read did not produce a view this panel can show, and why, in the panel's own
 * words — never by reading the message.
 *
 * Unlike a command's failure this carries no outcome, because a read has none:
 * it asked the gateway for nothing and changed nothing, so the only fact is that
 * the view on screen is older than the gateway's. Every rejected read arrives as
 * one of these, including the ones whose cause this build cannot name — an
 * unnamed cause is still a stale view, and {@link ReadFailure} has the word for
 * exactly that.
 */
export class ConversationReadFailedError extends Error {
  constructor(
    readonly reason: ReadFailure,
    cause?: unknown,
  ) {
    super(`The gateway did not answer with a view (${reason}).`, { cause })
    this.name = "ConversationReadFailedError"
  }
}

/**
 * What the gateway said became of a control that failed.
 *
 * Three states and not a flag, because a failed control has three honest
 * answers and two of them are certain. `refused` is the gateway deciding the
 * command before applying any of it — a code it rejects up front, or a review
 * whose option it reports as still pending. `applied` is the opposite
 * certainty: the review's option was consumed, so the choice took effect and
 * the command failed after that. `unknown` is the only one that leaves it open,
 * and is what the client's own sentence describes.
 *
 * A boolean here would have made `applied` and `unknown` the same answer, which
 * is the loss this type exists to prevent.
 */
export type ControlOutcome = "refused" | "applied" | "unknown"

/**
 * The gateway answered a conversation control with something worth passing on:
 * a reason this panel has a word for, a certain outcome, or both.
 *
 * Deliberately not named a refusal, because the reason alone does not say the
 * control was refused: `attachment-cleanup-unavailable` is a close that did
 * happen and whose release of the conversation's uploads did not. The two facts
 * do not determine each other — a review still pending is refused under any
 * code, including one this build has no word for — so both are carried, and the
 * store reads the conversation again regardless. It never replays a control.
 */
export class ControlFailedError extends Error {
  constructor(
    /** Undefined when the gateway's code has no word here; the outcome may still be certain. */
    readonly reason: CommandFailure | undefined,
    readonly outcome: ControlOutcome,
    cause?: unknown,
  ) {
    super(`The gateway could not complete this control (${reason ?? outcome}).`, {
      cause,
    })
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
