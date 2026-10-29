import {
  captureQuestionChoices,
  questionFields,
  type QuestionTarget,
  type QuestionChoice,
} from "../application/question-target.js"
import {
  executionFields,
  issueExecutionTarget,
  type ExecutionTarget,
} from "../application/execution-target.js"
import {
  NessaConversationMutationError,
  NessaConversationControlError,
} from "../application/conversation-mutation-error.js"
import type { RequestDeadline, RpcRequester } from "../application/session-port.js"
import {
  conversationDeleteTimeoutMs,
  conversationWorkspaceCreateTimeoutMs,
} from "../application/agent-budgets.js"
import { bounds, passiveReadTiming, ProductMethod } from "../generated/product.js"
import type {
  ConversationCreateResult,
  ConversationListResult,
  ConversationMessageDelivery,
  ConversationMessageReceipt,
  ConversationMessageStatusResult,
  ConversationObserveCursor,
  ConversationObserveResult,
  ConversationView,
  ConversationEarlier,
  ConversationPoint,
  ConversationPermission,
  ConversationQuestion,
  ConversationReceipt,
  ConversationMutationResult,
  ConversationQuestionChoice,
  ConversationPermissionRulesResult,
  ConversationReorderResult,
  ConversationRevokePermissionRuleResult,
  ConversationSetApprovalModeResult,
  ConversationSetUsageLimitAutoContinueResult,
  ConversationSetModelResult,
  ConversationCapability,
  ConversationShareResult,
  ConversationSharesCursor,
  ConversationSharesResult,
  ConversationWebhookCursorParams,
  ConversationWebhookCursorResult,
  ConversationWebhookResetResult,
  ConversationToolPolicy,
  ApprovalMode,
  PermissionRule,
  ImageAttachment,
  LinkedFile,
  UploadedFileAttachment,
} from "../generated/product.js"
import {
  imageAttachmentsProblem,
  linkedFilesProblem,
  uploadedFilesProblem,
} from "../protocol/attachment-validate.js"
import {
  conversationId,
  conversationIdPattern,
  conversationList,
  conversationMessageReceipt,
  conversationMessageStatus,
  conversationObserve,
  conversationView,
  conversationEarlier,
  conversationReceipt,
  conversationCommandReceipt,
  conversationMutation,
  conversationReorder,
  conversationApprovalMode,
  conversationUsageLimitAutoContinue,
  conversationModel,
  conversationShareResult,
  conversationShares,
  maxPolicyRevision,
  minExpectedPolicyRevision,
  conversationPermission,
  conversationQuestion,
  permissionReviewRange,
  reviewKeyPattern,
} from "../protocol/conversation-validate.js"
import { decimal } from "../protocol/passive-read-validate.js"
import { recordHead, validRecordScope } from "../protocol/record-read-validate.js"
import {
  permissionFields,
  type PermissionTarget,
} from "../application/permission-target.js"

/** Which of the caller's conversations `list()` returns. */
export type ConversationListOptions = {
  /** True for archived conversations only; false or omitted for the rest. */
  archived?: boolean
}

/** One page of `observe()`. */
export type ConversationObserveOptions = {
  /** True for archived conversations only; false or omitted for the rest. */
  archived?: boolean
  /** Absent on the first page. Later pages echo the cursor the previous page returned. */
  cursor?: ConversationObserveCursor
}

/** Optional caller-managed action identity. The client generates it when omitted. */
export type ConversationActionOptions = {
  /** Stable action attribution. Creation, message admission and the idempotent `releaseApp` control support same-ID retries; other controls require a fresh read and a new deliberate action. */
  requestId?: string
}
/** Optional IDs and delivery mode, filled by the client preparation wrapper. */
export type ConversationPrepareOptions = ConversationActionOptions & {
  /** Original stored inputs. Files are placed where the agent executes. */ uploads?: readonly UploadedFileAttachment[]
  /** Reuse only for the same logical message. */ executionId?: string
  /** Queued by default; steering uses the agent's supported behavior. */ mode?:
    "queued" | "steering"
}
/** A share or revoke, built on the grant as it was read. */
export type ConversationShareOptions = ConversationActionOptions & {
  /**
   * The `policyRevision` of the device's grant this change was built on, as
   * `shares()` listed it or an earlier `share()` answered it. The gateway
   * makes the change only while the grant is still at that revision, and
   * otherwise refuses it `share_grant_changed`, changing nothing. 0 names no
   * grant held (a revision starts at 1), so a first grant built on a list
   * without one is refused once another lands. Omitted, the change is made
   * whatever the device holds.
   */
  expectedPolicyRevision?: number
}
/** Create a named conversation, or omit its identity to generate a UUID. */
export type ConversationCreateOptions = ConversationActionOptions & {
  /** Stable conversation identity within the authenticated organization. */
  conversationId?: string
  /** The coding agent this conversation runs on for the rest of its life.
   * Omitted takes the gateway's own default. A conversation that already exists
   * reopens on the agent it was created with, whatever is passed here. A name
   * this gateway does not run is refused by the gateway, as a
   * NessaConversationMutationError with `uncertain` false. */
  agent?: string
  /** Initial catalog model when a new conversation is created. */
  model?: string
  /** Initial provider preset; omitted selects ask, or the channel's preset. */
  approvalMode?: ApprovalMode
  /** The channel this conversation is in for its whole life: one
   * `channels.list()` returned. Omitted is the default channel. Its agent,
   * model and preset fill in what is omitted here. One the gateway does not
   * name for this person is refused as `channel_not_found`, with `uncertain`
   * false. */
  channelId?: string
  /** The SSH destination this conversation's agent runs on for its whole
   * life: one `agents.list` returned under `environments`. Omitted runs it on
   * the gateway's own machine. One the gateway does not name is refused as
   * `environment_not_configured`, with `uncertain` false. */
  environment?: string
}
/** Message admission receipt with the client-owned action identity. */
export type ConversationSubmission = Readonly<ConversationReceipt> & {
  /** Original conversation identity. */ readonly conversationId: string
  /** Target owned before the admission request started. */ readonly target: ExecutionTarget
  /** Original action identifier. */ readonly requestId: string
}

/** Agent conversation commands. Creation and message admission failures expose NessaConversationMutationError.retry(). Controls expose NessaConversationControlError with uncertain effect status and require a fresh read before another deliberate action. Read returns a bounded full replacement view, suitable for serialized polling. */
/**
 * The title the gateway keeps for what a person typed to rename a
 * conversation, or `null` when nothing is left to call it: one line, every
 * run of whitespace or control characters one space, none at either end, cut
 * to the published maximum of code points. The gateway refuses any other
 * title (`invalid_request`), so pass this to `rename`. It folds at least what
 * the gateway calls whitespace or a control character, so what it returns
 * the gateway always takes.
 */
export function conversationTitle(text: string): string | null {
  const line = text.replace(/[\s\p{Cc}]+/gu, " ").trim()
  const cut = Array.from(line).slice(0, bounds.maxConversationRenameCharacters)
  return cut.join("").trimEnd() || null
}

export type ConversationApi = {
  /** Create or reopen a conversation. The gateway answers when the original attachment is ready. IDs are generated when options are omitted. */
  create: (options?: ConversationCreateOptions) => Promise<ConversationCreateResult>
  /** Change the provider preset using its native application boundary; pending reviews retain their identity. */
  setApprovalMode: (
    conversationId: string,
    mode: ApprovalMode,
    options?: ConversationActionOptions,
  ) => Promise<ConversationSetApprovalModeResult>
  /**
   * Turn this gateway's usage-limit continuation preference on or off.
   * The preference is shared by every conversation on this gateway.
   * Turning it on schedules a continuation for each currently blocked
   * eligible turn. Turning it off cancels open schedules.
   */
  setUsageLimitAutoContinue: (
    enabled: boolean,
    options?: ConversationActionOptions,
  ) => Promise<ConversationSetUsageLimitAutoContinueResult>
  /** Change model within this provider. Acknowledgement describes the native effect boundary. */
  setModel: (
    conversationId: string,
    model: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationSetModelResult>
  /**
   * Carry the conversation on under another agent and model. The next message
   * opens a new session of that agent, which is given the earlier turns as
   * text; `read()` lists the switch in `handoffs`, where the transcript draws
   * a line. Refused while a turn runs or waits, and for the agent the
   * conversation already runs on (use `setModel`). `applied` is false when
   * the switch was not made.
   */
  switchAgent: (
    conversationId: string,
    agent: string,
    model: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /**
   * Start a new conversation from a copy of `sourceConversationId`'s turns, on
   * its agent, model, approval preset and environment. The source must be
   * the caller's own. The new one's first `read()` lists the copied turns in
   * `handoffs`. IDs are generated when
   * options are omitted, and a retry of one request opens the same fork.
   */
  fork: (
    sourceConversationId: string,
    options?: Pick<ConversationCreateOptions, "conversationId" | "requestId">,
  ) => Promise<ConversationCreateResult>
  /** Read current provider lifecycle, output, queue, tools, and complete actionable permission choices. */
  read: (conversationId: string) => Promise<ConversationView>
  /**
   * Read what came before `before` in the conversation's saved history: the
   * turns nearest it that fit one frame, oldest first, ending where `before`
   * is. A view lists them only as `earlier`; its first message's start is
   * where to read before. With `after`, only what comes after that point, to
   * fill the gap between two points. A point the gateway's history does not
   * hold is refused as invalid input. Opens nothing.
   */
  readEarlier: (
    conversationId: string,
    before: ConversationPoint,
    after?: ConversationPoint,
  ) => Promise<ConversationEarlier>
  /**
   * List the conversations the authenticated caller owns, most recently
   * updated first, at most 500: each with its title, the last thing said in
   * it, when, and whether it is running. Closed conversations are included;
   * archived ones only when `options.archived` asks for them, and then only
   * they. Listing opens no provider, so it is cheap to call when a list is shown.
   */
  list: (options?: ConversationListOptions) => Promise<ConversationListResult>
  /**
   * Read one catalogue page of the conversations `list()` would name.
   * Walk the cursor until `complete` is true to see every stored summary
   * under that filter. A page with `complete: false` is not that set,
   * including when it has no cursor. The page holds at most one catalogue
   * page, not a larger list. Observing opens no provider.
   */
  observe: (options?: ConversationObserveOptions) => Promise<ConversationObserveResult>
  /** Fill optional identities and capture a complete message before effects.
   * Text is at most 8 KiB UTF-8 and may be blank only beside an image/file.
   * Images are references returned by attachments staging, not original bytes;
   * files are absolute paths on the gateway's machine. `options.uploads` names
   * original stored files delivered into the execution environment. The shared published
   * attachment validators own their bounds. Caller lists and entries are copied.
   * Throws TypeError before create or admission when any argument is invalid.
   */
  prepare: (
    conversationId: string,
    text: string,
    attachments?: readonly ImageAttachment[],
    files?: readonly LinkedFile[],
    options?: ConversationPrepareOptions,
  ) => PreparedSubmission
  /** Submit the exact immutable message prepared before any asynchronous effects. */
  submit: (prepared: PreparedSubmission) => Promise<ConversationSubmission>
  /**
   * Leave a note in this conversation: saved with who left it, it starts and
   * steers no turn. A `record_only` note is only kept; a `next_turn` note
   * waits for the next turn a person starts while the agent is idle, which
   * hands it to the model labelled with its author. The same `requestId`
   * with the same text and delivery answers the first note again, so a
   * failure's `retry()` saves it at most once.
   * @param text - 1 to 8 KiB UTF-8, not all blank, kept exactly.
   * @param delivery - `record_only` or `next_turn`.
   * @throws TypeError before anything is sent when the text breaks these bounds.
   */
  message: (
    conversationId: string,
    text: string,
    delivery: ConversationMessageDelivery,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMessageReceipt>
  /**
   * Where the caller's own note left under `requestId` stands now: pending,
   * taken by a turn (`executionId`), or retired (`retiredReason`). `found` is
   * false while no note is saved under it; send the same request again to
   * save it at most once. This writes nothing.
   */
  messageStatus: (
    conversationId: string,
    requestId: string,
  ) => Promise<ConversationMessageStatusResult>
  /** Remove the captured waiting input before provider dispatch. */
  remove: (
    target: ExecutionTarget,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /** Atomically reorder the complete waiting queue. Include every waiting execution once (at most 64); steering must remain ahead of ordinary input. A changed queue or priority conflict leaves it unchanged. Refresh after either outcome or an uncertain acknowledgement. */
  reorder: (
    conversationId: string,
    executionIds: readonly string[],
    options?: ConversationActionOptions,
  ) => Promise<ConversationReorderResult>
  /** Select an exact offered option for the identified pending review. */
  answer: (
    target: PermissionTarget,
    optionId: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /**
   * Read what one waiting review shows: who asked, its whole input, every
   * choice, and any deadline.
   *
   * A view lists each waiting review by its key alone. Read it here, and offer
   * no choice on it until this returns. Ranges of
   * `bounds.maxReviewRangeBytes` are asked one after another, each held to the
   * range asked; the assembled bytes are parsed only when they are the whole
   * length and UTF-8, and returned only when they are a valid review.
   * @param conversationId - The conversation whose view lists the review.
   * @param review - The review's key, as the view lists it.
   * @param options - `signal` abandons the read between ranges.
   * @returns The review, as its live owner holds it.
   * @throws The session's refusal, `stale_permission` once the review no
   * longer waits; `Error` for an answer that is not the range asked for or
   * not a review.
   */
  permissionReview: (
    conversationId: string,
    review: string,
    options?: { signal?: AbortSignal },
  ) => Promise<ConversationPermission>
  /**
   * Read all one waiting question asks: the agent's message and every
   * question, with its options.
   *
   * A view lists each waiting question by a small entry under its key. Read
   * it here, and offer no answer to it until this returns; it is read as
   * {@link ConversationApi.permissionReview} reads a review.
   * @param conversationId - The conversation whose view lists the question.
   * @param question - The question's key, as the view lists it.
   * @param options - `signal` abandons the read between ranges.
   * @returns The question, as the agent's live invocation holds it.
   * @throws The session's refusal, `stale_permission` once the question no
   * longer waits; `Error` for an answer that is not the range asked for or
   * not that question.
   */
  questionReview: (
    conversationId: string,
    question: string,
    options?: { signal?: AbortSignal },
  ) => Promise<ConversationQuestion>
  /**
   * Answer one question the agent asked, or decline it.
   *
   * `choices` of null declines: the agent is told it was asked and answered
   * with nothing, which is an answer rather than silence. A question left out
   * of `choices` is skipped, which every ask permits.
   * A failed acknowledgement is a control error without replay; read the
   * current question before making a new deliberate answer.
   */
  answerQuestion: (
    target: QuestionTarget,
    choices: readonly QuestionChoice[] | null,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /** Withdraw the identified review with a human-readable reason recorded by the gateway. */
  cancel: (
    target: PermissionTarget,
    reason: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /** Close the provider context and stop admitted work; saved conversation history remains. */
  close: (
    conversationId: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /** Stop the named turn. A queued turn is withdrawn and the active turn is cancelled. The attachment stays open. */
  stop: (
    target: ExecutionTarget,
    options?: ConversationActionOptions,
  ) => Promise<{ requestId: string; stage: string; outcome?: string }>
  /** Archive a conversation: `list()` stops showing it unless archived ones are asked for. Nothing is stopped or removed, and a new message unarchives it. `applied` is false when it was already archived, or when the gateway has no summary for it (nothing was said in it, or its summary was never written) — such a conversation is never listed, so there is nothing to archive. */
  archive: (
    conversationId: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /** Undo `archive`. `applied` is false when it was not archived. */
  unarchive: (
    conversationId: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /** Rename a conversation: `list()` and `read()` show `title` from now on, and a later message does not rename it back. `title` must be one the gateway keeps, as `conversationTitle` makes it; the gateway refuses anything else with `invalid_request`. `applied` is false when it already has that title, or when the gateway has no summary for it, as for `archive`. */
  rename: (
    conversationId: string,
    title: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /**
   * The owner's remembered permission rules, in the order they were stored.
   * A shared user is refused. A rule is one tool in one workspace.
   */
  permissionRules: (
    options?: ConversationActionOptions,
  ) => Promise<ConversationPermissionRulesResult>
  /**
   * Delete one remembered rule. `applied` is false when no rule has that id.
   * A shared user is refused. Later reviews of that tool ask again.
   */
  revokePermissionRule: (
    ruleId: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationRevokePermissionRuleResult>
  /**
   * Delete a conversation permanently: the gateway stops it, asks its agent to
   * delete the agent's own session, records who deleted it, and erases its
   * history, uploads and summary; audit evidence is kept. Every later command
   * on it rejects with `conversation_deleted` — except its owner deleting it
   * again, which resolves with `applied: true` for a repeat of the deciding
   * request (same caller, surface and `requestId`) and `applied: false`
   * otherwise — so a surface holding it
   * should let it go; anyone else is told `conversation_not_found`. A rejection
   * with `conversation_erasure_incomplete` means it *was* deleted and some
   * stored data remains; one with `audit_unavailable` means it *was* deleted
   * and what did not finish is the record of it — the deletion record, or the
   * uploads' own evidence. Any other rejection from a delete means only that
   * whether it was deleted is not known: list it, or delete again. Deleting again, and each gateway start, tries the
   * erasure again; an agent that keeps refusing to delete its own session, or
   * a damaged history, needs the operator.
   */
  delete: (
    conversationId: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /**
   * Grant one paired device or peer gateway a capability set on one of the
   * caller's conversations, or change the capabilities and constraints it
   * holds there (`conversation.share`, `credential.manage`). `credentialId`
   * is one `client.credentials.list()` names. The only sets are `read`,
   * `read` and `comment`, and those plus `drive`. A set that includes
   * `drive` needs `policy`; the others take none, and the gateway refuses
   * either mistake `invalid_request`, as it does a set that is not one of
   * those three and a policy past the schema's bounds (published in
   * `bounds`: `maxToolPolicyPatterns`, `maxToolPatternBytes`,
   * `maxCommandHosts`, `maxCommandHostBytes`). Comment and drive for a
   * gateway principal are refused `capabilities_refused`. `details.policies`
   * names the Cedar policy ids, and is empty when no permit matched. Nothing
   * is written. A grant that names no `commandHosts` runs commands nowhere.
   * The answer names the grant's `policyRevision`, the grant epoch, once the
   * share is made. `applied` is false when that same receiver and credential
   * already held those capabilities and constraints, and the epoch is kept.
   * Replacing the receiver or the credential applies and moves the catalogue
   * revision. The epoch stays when the capabilities and constraints do.
   * With `expectedPolicyRevision`, a grant no longer at that revision is
   * refused `share_grant_changed`. The same `requestId` answers what it
   * answered first, its revision included, so a failure's `retry()` grants at
   * most once.
   * @throws TypeError before anything is sent for a malformed conversation ID,
   * a blank or over-long credential or request ID, or an expected revision
   * that is not a whole number from 0 to 2^53 - 1.
   */
  share: (
    conversationId: string,
    credentialId: string,
    capabilities: readonly ConversationCapability[],
    policy?: ConversationToolPolicy,
    options?: ConversationShareOptions,
  ) => Promise<ConversationShareResult>
  /**
   * Revoke one paired party's grant on one of the caller's conversations,
   * whatever capabilities it holds (`conversation.unshare`). It needs only ownership, so it
   * works on a deleted conversation and for a party since unpaired. `applied`
   * is false when no grant was held; either way none is held afterwards. With
   * `expectedPolicyRevision`, a grant no longer at that revision is refused
   * `share_grant_changed`. Retried under the same `requestId`, it answers
   * what it answered first.
   * @throws TypeError as `share` does.
   */
  unshare: (
    conversationId: string,
    credentialId: string,
    options?: ConversationShareOptions,
  ) => Promise<ConversationMutationResult>
  /**
   * One page of the grants on one of the caller's conversations, oldest first
   * (`conversation.shares`). `pageSize` is a whole number from 1 through the
   * published page size (`sharePage.pageSize`). The first page names no
   * cursor. A later page echoes the cursor the previous page returned, and is
   * the grants after that one. The answer's cursor is present when further
   * grants remain, and names that page's last grant. Every page names the
   * catalogue revision (`changeRevision`) it was read at. A conversation holds
   * at most `sharePage.grants`. An answer with more grants than the page size
   * this call sent is refused, the same as any other answer the client will
   * not read.
   * @throws TypeError before anything is sent for a malformed conversation ID,
   * a page size outside that range, or a cursor whose credential or time is
   * not one the schema takes.
   */
  shares: (
    conversationId: string,
    options: { pageSize: number; cursor?: ConversationSharesCursor },
  ) => Promise<ConversationSharesResult>
  /** Read the owner's registered webhook checkpoint. Optional source Head follows
   * the native snapshot as a separate, non-atomic observation; compare the whole
   * scope before comparing positions. Omitted/false performs no source I/O.
   * Its requestId is available only while frozen and is the server-issued reset
   * identity; a scope in this result grants no receiver/read/send authority.
   */
  webhookCursor: (
    conversationId: string,
    credentialId: string,
    options?: Omit<ConversationWebhookCursorParams, "conversationId" | "credentialId">,
  ) => Promise<ConversationWebhookCursorResult>
  /** Select an observed incarnation for one frozen server-issued capture.
   * Explicit retries use that same requestId and receive the original receipt;
   * they do not rewind again. Fresh server authorization applies to every call.
   * The result is not a current-status or resumed-delivery promise.
   */
  resetWebhookCursor: (
    conversationId: string,
    credentialId: string,
    requestId: string,
  ) => Promise<ConversationWebhookResetResult>
}

const utf8 = new TextEncoder()
/** The expected revision a share or revoke sends, when it names one: a whole number the schema takes. */
function expectedRevision(options: ConversationShareOptions): {
  expectedPolicyRevision?: number
} {
  const expected = options.expectedPolicyRevision
  if (expected === undefined) return {}
  if (
    !Number.isInteger(expected) ||
    expected < minExpectedPolicyRevision ||
    expected > maxPolicyRevision
  )
    throw new TypeError(
      "Expected policy revision must be a whole number from 0 to 2^53 - 1",
    )
  return { expectedPolicyRevision: expected }
}
/** The page size `conversation.shares` sends: a whole number the schema takes. */
function sharePageSize(pageSize: number): number {
  if (!Number.isInteger(pageSize) || pageSize < 1 || pageSize > bounds.maxSharePage)
    throw new TypeError(
      `Page size must be a whole number from 1 to ${bounds.maxSharePage}`,
    )
  return pageSize
}

/** The cursor a later shares page echoes, copied so the caller cannot change it after. */
function shareCursor(cursor: ConversationSharesCursor | undefined): {
  cursor?: ConversationSharesCursor
} {
  if (cursor === undefined) return {}
  boundedText(cursor.credentialId, "Credential ID", 256)
  if (!Number.isSafeInteger(cursor.grantedAtMs) || cursor.grantedAtMs < 0)
    throw new TypeError("Cursor time must be a whole number from 0 to 2^53 - 1")
  return {
    cursor: { credentialId: cursor.credentialId, grantedAtMs: cursor.grantedAtMs },
  }
}

function boundedText(value: string, name: string, maxBytes: number): string {
  if (
    typeof value !== "string" ||
    !value.trim() ||
    utf8.encode(value).byteLength > maxBytes
  )
    throw new TypeError(`${name} must contain 1-${maxBytes} UTF-8 bytes`)
  return value
}
function observeDecimal(value: string): string {
  if (!decimal(value)) throw new TypeError("Invalid observation cursor")
  return value
}
function validConversationId(value: string): string {
  if (typeof value !== "string" || !conversationIdPattern.test(value))
    throw new TypeError("Conversation ID must be a canonical lowercase UUID")
  return value
}

/** Complete logical message intent before create/admission starts. */
export type SubmissionInput = {
  readonly conversationId: string
  readonly executionId: string
  readonly requestId: string
  readonly mode: "queued" | "steering"
  readonly text: string
  readonly attachments?: readonly ImageAttachment[]
  readonly files?: readonly LinkedFile[]
  readonly uploads?: readonly UploadedFileAttachment[]
}
declare const preparedSubmission: unique symbol
/** Process-local immutable message and its captured control target. */
export type PreparedSubmission = Omit<
  SubmissionInput,
  "attachments" | "files" | "uploads"
> & {
  readonly attachments: readonly Readonly<ImageAttachment>[]
  readonly files: readonly Readonly<LinkedFile>[]
  readonly uploads: readonly Readonly<UploadedFileAttachment>[]
  readonly target: ExecutionTarget
  readonly [preparedSubmission]: true
}
const preparedValues = new WeakSet<PreparedSubmission>()

/** Validate and copy one complete message; no external effect runs here. */
export function prepareSubmission(input: SubmissionInput): PreparedSubmission {
  const { conversationId, text, mode } = input
  const attachments = input.attachments ?? []
  const linked = input.files ?? []
  const inputUploads = input.uploads ?? []
  validConversationId(conversationId)
  if (mode !== "queued" && mode !== "steering")
    throw new TypeError("Invalid submission mode")
  const executionId = boundedText(input.executionId, "Execution ID", 256)
  const requestId = boundedText(input.requestId, "Request ID", 256)
  // Capture into owned plain arrays, not caller slice/map/iterator results.
  // Bounds cap the copy before any entry is retained; final validation remains
  // with the published attachment validators (PS19).
  const imageCount = Array.isArray(attachments) ? attachments.length : NaN
  const fileCount = Array.isArray(linked) ? linked.length : NaN
  if (
    !Number.isSafeInteger(imageCount) ||
    imageCount < 0 ||
    imageCount > bounds.maxMessageImages
  )
    throw new TypeError("Invalid message attachments: too many images or not a list")
  if (
    !Number.isSafeInteger(fileCount) ||
    fileCount < 0 ||
    fileCount > bounds.maxMessageFiles
  )
    throw new TypeError("Invalid message files: too many files or not a list")
  if (
    !Array.isArray(inputUploads) ||
    inputUploads.length + fileCount > bounds.maxMessageFiles
  )
    throw new TypeError("Invalid message uploads: too many files")
  const uploads: Readonly<UploadedFileAttachment>[] = []
  for (let index = 0; index < inputUploads.length; index++) {
    const item = inputUploads[index]
    uploads.push(
      item && typeof item === "object" && !Array.isArray(item)
        ? Object.freeze({ ...item })
        : item,
    )
  }
  const uploadProblem = uploadedFilesProblem(uploads)
  if (uploadProblem) throw new TypeError(`Invalid message uploads: ${uploadProblem}`)
  const images: Readonly<ImageAttachment>[] = []
  const files: Readonly<LinkedFile>[] = []
  for (let index = 0; index < imageCount; index++) {
    const item = attachments[index]
    images.push(
      item && typeof item === "object" && !Array.isArray(item)
        ? Object.freeze({ ...item })
        : item,
    )
  }
  for (let index = 0; index < fileCount; index++) {
    const item = linked[index]
    files.push(
      item && typeof item === "object" && !Array.isArray(item)
        ? Object.freeze({ ...item })
        : item,
    )
  }
  const problem = imageAttachmentsProblem(images)
  if (problem) throw new TypeError(`Invalid message attachments: ${problem}`)
  const filesProblem = linkedFilesProblem(files)
  if (filesProblem) throw new TypeError(`Invalid message files: ${filesProblem}`)
  // The copied payload decides whether text may be blank, not a changing source.
  if (images.length === 0 && files.length === 0 && uploads.length === 0)
    boundedText(text, "Message", 8192)
  else if (typeof text !== "string" || utf8.encode(text).byteLength > 8192)
    throw new TypeError("Message must contain at most 8192 UTF-8 bytes")
  const prepared = Object.freeze({
    conversationId,
    executionId,
    requestId,
    mode,
    text,
    attachments: Object.freeze(images),
    files: Object.freeze(files),
    uploads: Object.freeze(uploads),
    target: issueExecutionTarget(conversationId, executionId),
  }) as PreparedSubmission
  preparedValues.add(prepared)
  return prepared
}

/** Check a prepared value before a substitute or transport consumes it. */
export function preparedSubmissionFields(
  prepared: PreparedSubmission,
): PreparedSubmission {
  if (!preparedValues.has(prepared))
    throw new TypeError("Use an original prepared submission")
  return prepared
}

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null
    ? (value as Record<string, unknown>)
    : null
}

function field(value: Record<string, unknown>, name: string): unknown {
  return Object.hasOwn(value, name) ? value[name] : undefined
}

function rememberedRules(
  value: unknown,
  requestId: string,
): ConversationPermissionRulesResult {
  const body = record(value)
  const rules = body && field(body, "rules")
  if (!body || field(body, "requestId") !== requestId || !Array.isArray(rules))
    throw new TypeError("Invalid permission rules")
  return {
    requestId,
    rules: rules.map((rule) => {
      const row = record(rule)
      const effect = row && field(row, "effect")
      const source = row && field(row, "source")
      if (
        !row ||
        typeof field(row, "id") !== "string" ||
        typeof field(row, "revision") !== "string" ||
        typeof field(row, "workspace") !== "string" ||
        (source !== "claude" && source !== "codex") ||
        typeof field(row, "tool") !== "string" ||
        (effect !== "allow" && effect !== "deny")
      )
        throw new TypeError("Invalid permission rule")
      const listed: PermissionRule = {
        id: field(row, "id") as string,
        revision: field(row, "revision") as string,
        workspace: field(row, "workspace") as string,
        source,
        tool: field(row, "tool") as string,
        effect,
      }
      return listed
    }),
  }
}

function revokedRule(
  value: unknown,
  requestId: string,
): ConversationRevokePermissionRuleResult {
  const body = record(value)
  const applied = body && field(body, "applied")
  if (!body || field(body, "requestId") !== requestId || typeof applied !== "boolean")
    throw new TypeError("Invalid permission rule revocation")
  return { requestId, applied }
}

export function createConversationApi(
  session: RpcRequester,
  newId: () => string,
): ConversationApi {
  function mutate<T>(
    method: string,
    params: { conversationId: string; requestId: string; executionId?: string } & Record<
      string,
      | string
      | readonly string[]
      | readonly ImageAttachment[]
      | readonly LinkedFile[]
      | readonly UploadedFileAttachment[]
      | readonly QuestionChoice[]
      | readonly ConversationQuestionChoice[]
      | ConversationToolPolicy
      | number
      | null
      | undefined
    >,
    validate: (value: unknown) => T,
    retryable = true,
    permissionAnswer = false,
    deadline?: RequestDeadline,
  ): Promise<T> {
    const command = Object.freeze({ ...params })
    const perform = async (): Promise<T> => {
      try {
        return validate(
          await (deadline === undefined
            ? session.request(method, command)
            : session.request(method, command, deadline)),
        )
      } catch (cause) {
        if (!retryable) {
          throw new NessaConversationControlError(
            command.conversationId,
            command.requestId,
            command.executionId,
            cause,
            permissionAnswer,
          )
        }
        throw new NessaConversationMutationError(
          command.conversationId,
          command.requestId,
          command.executionId,
          cause,
          perform,
        )
      }
    }
    return perform()
  }
  function action(
    method: string,
    conversationId: string,
    fields: Record<string, string>,
    options: ConversationActionOptions = {},
    permissionAnswer = false,
    deadline?: RequestDeadline,
  ) {
    validConversationId(conversationId)
    const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
    for (const [name, value] of Object.entries(fields))
      boundedText(value, name, name === "reason" ? 1024 : 256)
    return mutate(
      method,
      { conversationId, requestId, ...fields },
      (value) => conversationMutation(value, requestId),
      false,
      permissionAnswer,
      deadline,
    )
  }
  /**
   * The whole of what one listed review or question shows, read range by
   * range from its live owner with `method`, `key` naming it, each range held
   * to the one asked; parsed only once whole and UTF-8.
   */
  async function readWhole(
    method: string,
    conversationId: string,
    key: { review: string } | { question: string },
    signal?: AbortSignal,
  ): Promise<unknown> {
    validConversationId(conversationId)
    if (!reviewKeyPattern.test(Object.values(key)[0] as string))
      throw new TypeError("Use a key from a view")
    const maxBytes = bounds.maxReviewRangeBytes
    const parts: Uint8Array[] = []
    let offset = 0
    let totalBytes: number | undefined
    do {
      signal?.throwIfAborted()
      const range = permissionReviewRange(
        await session.request(method, { conversationId, ...key, offset, maxBytes }),
        offset,
        maxBytes,
        totalBytes,
      )
      totalBytes = range.totalBytes
      parts.push(range.bytes)
      offset += range.bytes.byteLength
    } while (offset < totalBytes)
    const whole = new Uint8Array(offset)
    let at = 0
    for (const part of parts) {
      whole.set(part, at)
      at += part.byteLength
    }
    return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(whole))
  }
  return {
    create: (options = {}) => {
      const id = validConversationId(options.conversationId ?? newId())
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      // An agent name the gateway does not know is the gateway's to refuse, and
      // it already refuses one: an unparseable name becomes `RequestedAgent::
      // Unknown`, which is `invalid_request` on the wire and so arrives here as
      // a `NessaConversationMutationError` with `uncertain` false and a working
      // `retry()`, like every other pre-admission refusal. Bounding the name
      // here instead sorted the blank and the over-long out of that answer and
      // into a bare `TypeError` thrown before `mutate` is even entered: no
      // `uncertain`, no `retry()`, no reason — and thrown for a value that is
      // usually remembered rather than typed, which made one bad remembered
      // name fail the launch. One refuser, one shape of refusal.
      const agent = options.agent === undefined ? {} : { agent: options.agent }
      const model = options.model === undefined ? {} : { model: options.model }
      const approvalMode =
        options.approvalMode === undefined ? {} : { approvalMode: options.approvalMode }
      const environment =
        options.environment === undefined ? {} : { environment: options.environment }
      const channelId =
        options.channelId === undefined ? {} : { channelId: options.channelId }
      return mutate(
        ProductMethod.ConversationCreate,
        {
          conversationId: id,
          requestId,
          ...agent,
          ...model,
          ...approvalMode,
          ...environment,
          ...channelId,
        },
        (value) => conversationId(value, id),
        true,
        false,
        options.channelId !== undefined || options.environment === "boat:thread"
          ? { atLeastMs: conversationWorkspaceCreateTimeoutMs }
          : undefined,
      )
    },
    setModel: (id, model, options = {}) => {
      validConversationId(id)
      boundedText(model, "Model", 256)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      return mutate(
        ProductMethod.ConversationSetModel,
        { conversationId: id, requestId, model },
        (value) => conversationModel(value, requestId, model),
        false,
      )
    },
    switchAgent: (id, agent, model, options = {}) => {
      validConversationId(id)
      boundedText(model, "Model", 256)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      // The agent name is the gateway's to refuse, as for `create`.
      return mutate(
        ProductMethod.ConversationSwitchAgent,
        { conversationId: id, requestId, agent, model },
        (value) => conversationMutation(value, requestId),
        false,
      )
    },
    fork: (source, options = {}) => {
      validConversationId(source)
      const id = validConversationId(options.conversationId ?? newId())
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      return mutate(
        ProductMethod.ConversationFork,
        { conversationId: id, requestId, sourceConversationId: source },
        (value) => conversationId(value, id),
      )
    },
    setApprovalMode: (id, mode, options = {}) => {
      validConversationId(id)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      return mutate(
        ProductMethod.ConversationSetApprovalMode,
        { conversationId: id, requestId, mode },
        (value) => conversationApprovalMode(value, requestId, mode),
        false,
      )
    },
    setUsageLimitAutoContinue: async (enabled, options = {}) => {
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      return conversationUsageLimitAutoContinue(
        await session.request(ProductMethod.ConversationSetUsageLimitAutoContinue, {
          requestId,
          enabled,
        }),
        requestId,
        enabled,
      )
    },
    list: async (options = {}) =>
      conversationList(
        await session.request(
          ProductMethod.ConversationList,
          options.archived === undefined ? {} : { archived: options.archived },
        ),
        options.archived ?? false,
      ),
    observe: async (options = {}) => {
      const cursor =
        options.cursor === undefined
          ? {}
          : {
              cursor: {
                incarnation: boundedText(
                  options.cursor.incarnation,
                  "Incarnation",
                  bounds.maxSyncIdBytes,
                ),
                boundary: observeDecimal(options.cursor.boundary),
                creation: observeDecimal(options.cursor.creation),
                id: validConversationId(options.cursor.id),
              },
            }
      return conversationObserve(
        await session.request(ProductMethod.ConversationObserve, {
          ...(options.archived === undefined ? {} : { archived: options.archived }),
          ...cursor,
        }),
        options.archived ?? false,
      )
    },
    read: async (id) =>
      conversationView(
        await session.request(ProductMethod.ConversationRead, {
          conversationId: validConversationId(id),
        }),
        id,
      ),
    readEarlier: async (id, before, after) =>
      conversationEarlier(
        await session.request(ProductMethod.ConversationReadEarlier, {
          conversationId: validConversationId(id),
          before,
          ...(after === undefined ? {} : { after }),
        }),
        id,
      ),
    prepare: (conversationId, text, attachments, files, options = {}) =>
      prepareSubmission({
        conversationId,
        text,
        attachments,
        files,
        uploads: options.uploads,
        mode: options.mode ?? "queued",
        executionId: options.executionId ?? newId(),
        requestId: options.requestId ?? newId(),
      }),
    submit: (prepared) => {
      const {
        conversationId,
        executionId,
        requestId,
        text,
        attachments,
        files,
        mode,
        uploads,
        target,
      } = preparedSubmissionFields(prepared)
      return mutate(
        mode === "steering"
          ? ProductMethod.ConversationSteer
          : ProductMethod.ConversationSend,
        {
          conversationId,
          executionId,
          requestId,
          text,
          attachments,
          files,
          ...(uploads.length ? { uploads } : {}),
        },
        (value) =>
          Object.freeze({
            ...conversationReceipt(value, executionId),
            conversationId,
            requestId,
            target,
          }),
      )
    },
    message: (id, text, delivery, options = {}) => {
      validConversationId(id)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      boundedText(text, "Note", 8192)
      if (delivery !== "record_only" && delivery !== "next_turn")
        throw new TypeError("Note delivery must be record_only or next_turn")
      return mutate(
        ProductMethod.ConversationMessage,
        { conversationId: id, requestId, text, delivery },
        (value) => conversationMessageReceipt(value, requestId),
      )
    },
    messageStatus: async (id, requestId) =>
      conversationMessageStatus(
        await session.request(ProductMethod.ConversationMessageStatus, {
          conversationId: validConversationId(id),
          requestId: boundedText(requestId, "Request ID", 256),
        }),
        requestId,
      ),
    remove: (target, options) => {
      const { conversationId, executionId } = executionFields(target)
      return action(
        ProductMethod.ConversationRemove,
        conversationId,
        { executionId },
        options,
      )
    },
    reorder: async (id, executionIds, options = {}) => {
      validConversationId(id)
      if (
        executionIds.length > 64 ||
        new Set(executionIds).size !== executionIds.length ||
        executionIds.some(
          (value) =>
            typeof value !== "string" ||
            !value.trim() ||
            utf8.encode(value).byteLength > 256,
        )
      ) {
        throw new TypeError("Queue order requires at most 64 unique execution IDs")
      }
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      return mutate(
        ProductMethod.ConversationReorder,
        { conversationId: id, requestId, executionIds: Object.freeze([...executionIds]) },
        (value) => conversationReorder(value, requestId),
        false,
      )
    },
    answerQuestion: (target, choices, options = {}) => {
      const capturedChoices = captureQuestionChoices(choices)
      const {
        conversationId: id,
        question,
        executionId,
        questionId,
      } = questionFields(target, capturedChoices)
      validConversationId(id)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      return mutate(
        ProductMethod.ConversationAnswerQuestion,
        {
          conversationId: id,
          requestId,
          executionId,
          question,
          questionId,
          choices: capturedChoices,
        },
        (value) => conversationMutation(value, requestId),
        false,
      )
    },
    answer: (target, optionId, options) => {
      const { conversationId, review, executionId, permissionId } = permissionFields(
        target,
        optionId,
      )
      return action(
        ProductMethod.ConversationAnswer,
        conversationId,
        { executionId, review, permissionId, optionId },
        options,
        true,
      )
    },
    permissionReview: async (conversationId, review, options = {}) => {
      const shown = conversationPermission(
        await readWhole(
          ProductMethod.ConversationPermissionReview,
          conversationId,
          { review },
          options.signal,
        ),
      )
      if (shown.review !== review) throw new Error("Review response is another review")
      return shown
    },
    questionReview: async (conversationId, question, options = {}) => {
      const shown = conversationQuestion(
        await readWhole(
          ProductMethod.ConversationQuestionReview,
          conversationId,
          { question },
          options.signal,
        ),
      )
      if (shown.question !== question)
        throw new Error("Question response is another question")
      return shown
    },
    cancel: (target, reason, options) => {
      const { conversationId, review, executionId, permissionId } =
        permissionFields(target)
      return action(
        ProductMethod.ConversationCancel,
        conversationId,
        { executionId, review, permissionId, reason },
        options,
      )
    },
    close: (id, options) => action(ProductMethod.ConversationClose, id, {}, options),
    stop: (target, options = {}) => {
      const { conversationId: id, executionId } = executionFields(target)
      validConversationId(id)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      boundedText(executionId, "Execution ID", 256)
      return mutate(
        ProductMethod.ConversationStop,
        { conversationId: id, requestId, executionId },
        (value) => conversationCommandReceipt(value, requestId),
        false,
      )
    },
    permissionRules: async (options = {}) => {
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      return rememberedRules(
        await session.request(ProductMethod.ConversationPermissionRules, { requestId }),
        requestId,
      )
    },
    revokePermissionRule: async (ruleId, options = {}) => {
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      boundedText(ruleId, "Rule ID", 256)
      return revokedRule(
        await session.request(ProductMethod.ConversationRevokePermissionRule, {
          requestId,
          ruleId,
        }),
        requestId,
      )
    },
    archive: (id, options) => action(ProductMethod.ConversationArchive, id, {}, options),
    unarchive: (id, options) =>
      action(ProductMethod.ConversationUnarchive, id, {}, options),
    // Not through `action`: its text bound is not a title's, and the gateway
    // owns what a title may be.
    rename: (id, title, options = {}) => {
      validConversationId(id)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      return mutate(
        ProductMethod.ConversationRename,
        { conversationId: id, requestId, title },
        (value) => conversationMutation(value, requestId),
        false,
      )
    },
    delete: (id, options) =>
      action(ProductMethod.ConversationDelete, id, {}, options, false, {
        atLeastMs: conversationDeleteTimeoutMs,
      }),
    share: (id, credentialId, capabilities, policy, options = {}) => {
      validConversationId(id)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      boundedText(credentialId, "Credential ID", 256)
      // Copied, so a caller changing its policy or the set between a failure
      // and its retry cannot send another grant under the same request ID.
      const frozen =
        policy === undefined
          ? undefined
          : (JSON.parse(JSON.stringify(policy)) as ConversationToolPolicy)
      const granted = [...capabilities]
      return mutate(
        ProductMethod.ConversationShare,
        {
          conversationId: id,
          requestId,
          credentialId,
          capabilities: granted,
          ...(frozen === undefined ? {} : { policy: frozen }),
          ...expectedRevision(options),
        },
        (value) => conversationShareResult(value, requestId),
      )
    },
    unshare: (id, credentialId, options = {}) => {
      validConversationId(id)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      boundedText(credentialId, "Credential ID", 256)
      return mutate(
        ProductMethod.ConversationUnshare,
        { conversationId: id, requestId, credentialId, ...expectedRevision(options) },
        (value) => conversationMutation(value, requestId),
      )
    },
    webhookCursor: async (id, credentialId, options = {}) => {
      validConversationId(id)
      boundedText(credentialId, "Credential ID", 256)
      const includeSourceHead = options.includeSourceHead
      if (includeSourceHead !== undefined && typeof includeSourceHead !== "boolean")
        throw new TypeError("includeSourceHead must be boolean")
      const params = {
        conversationId: id,
        credentialId,
        ...(includeSourceHead === undefined ? {} : { includeSourceHead }),
      }
      const value = includeSourceHead
        ? await session.request(ProductMethod.ConversationWebhookCursor, params, {
            atLeastMs: passiveReadTiming.minRequestTimeoutMs,
          })
        : await session.request(ProductMethod.ConversationWebhookCursor, params)
      return webhookCursorResult(value, id, includeSourceHead === true)
    },
    resetWebhookCursor: (id, credentialId, requestId) => {
      validConversationId(id)
      boundedText(credentialId, "Credential ID", 256)
      if (typeof requestId !== "string" || !conversationIdPattern.test(requestId))
        throw new TypeError("Reset request ID must be the server-issued canonical UUID")
      return mutate(
        ProductMethod.ConversationWebhookReset,
        { conversationId: id, credentialId, requestId },
        (value) => webhookResetResult(value, id, requestId),
      )
    },
    shares: async (id, options) => {
      const pageSize = sharePageSize(options.pageSize)
      const page = conversationShares(
        await session.request(ProductMethod.ConversationShares, {
          conversationId: validConversationId(id),
          pageSize,
          ...shareCursor(options.cursor),
        }),
      )
      // The schema allows up to the published page size. This call asked for
      // `pageSize`, and an answer past that is not the page it asked for.
      if (page.items.length > pageSize)
        throw new Error("Invalid conversation shares page")
      return page
    },
  }
}

function webhookCursorResult(
  value: unknown,
  conversation: string,
  includeSourceHead: boolean,
): ConversationWebhookCursorResult {
  const item = record(value)
  if (
    !item ||
    Object.keys(item).length !== (includeSourceHead ? 5 : 4) ||
    !validRecordScope(item.scope) ||
    item.scope.stream !== conversation ||
    !decimal(item.position) ||
    typeof item.resetRequired !== "boolean" ||
    (item.resetRequired
      ? typeof item.requestId !== "string" || !conversationIdPattern.test(item.requestId)
      : item.requestId !== null)
  ) {
    throw new TypeError("Invalid registered webhook checkpoint")
  }
  if (includeSourceHead) {
    const head = recordHead(item.observedSourceHead)
    if (
      head.scope.stream !== conversation ||
      head.scope.receiver !== item.scope.receiver ||
      head.scope.accessEpoch !== item.scope.accessEpoch
    )
      throw new TypeError("Invalid registered webhook source selector")
  }
  return item as unknown as ConversationWebhookCursorResult
}
function webhookResetResult(
  value: unknown,
  conversation: string,
  requestId: string,
): ConversationWebhookResetResult {
  const item = record(value)
  if (
    !item ||
    Object.keys(item).length !== 5 ||
    item.requestId !== requestId ||
    typeof item.receiptId !== "string" ||
    !conversationIdPattern.test(item.receiptId) ||
    item.receiptId === requestId ||
    !validRecordScope(item.scope) ||
    item.scope.stream !== conversation ||
    item.position !== "0" ||
    !Number.isSafeInteger(item.appliedAtMs) ||
    (item.appliedAtMs as number) < 0
  ) {
    throw new TypeError("Invalid observed-incarnation reset receipt")
  }
  return item as unknown as ConversationWebhookResetResult
}
