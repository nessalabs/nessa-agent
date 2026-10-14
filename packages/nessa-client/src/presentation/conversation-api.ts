import {
  captureQuestionChoices,
  questionFields,
  type QuestionTarget,
  type QuestionChoice,
} from "../application/question-target.js"
import {
  NessaConversationMutationError,
  NessaConversationControlError,
} from "../application/conversation-mutation-error.js"
import type { RequestDeadline, RpcRequester } from "../application/session-port.js"
import { conversationDeleteTimeoutMs } from "../application/agent-budgets.js"
import { bounds, ProductMethod } from "../generated/product.js"
import type {
  ConversationCreateResult,
  ConversationListResult,
  ConversationMessageDelivery,
  ConversationMessageReceipt,
  ConversationMessageStatusResult,
  ConversationObserveCursor,
  ConversationObserveResult,
  ConversationView,
  ConversationReceipt,
  ConversationMutationResult,
  ConversationQuestionChoice,
  ConversationReorderResult,
  ConversationSetApprovalModeResult,
  ConversationShareResult,
  ConversationShareRole,
  ConversationSharesCursor,
  ConversationSharesResult,
  ConversationToolPolicy,
  ApprovalMode,
  ImageAttachment,
  LinkedFile,
} from "../generated/product.js"
import {
  imageAttachmentsProblem,
  linkedFilesProblem,
} from "../protocol/attachment-validate.js"
import {
  conversationId,
  conversationIdPattern,
  conversationList,
  conversationMessageReceipt,
  conversationMessageStatus,
  conversationObserve,
  conversationView,
  conversationReceipt,
  conversationCommandReceipt,
  conversationMutation,
  conversationReorder,
  conversationApprovalMode,
  conversationShareResult,
  conversationShares,
  maxPolicyRevision,
  minExpectedPolicyRevision,
} from "../protocol/conversation-validate.js"
import { decimal } from "../protocol/passive-read-validate.js"
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
/** Optional identities for optimistic display or explicit serializable retry state. */
export type ConversationSendOptions = ConversationActionOptions & {
  /** Reuse only for a retry of the same message. Generated when omitted. */
  executionId?: string
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
  /** Exact catalog model fixed when a new conversation is created. */
  model?: string
  /** Initial provider preset; omitted selects ask. */
  approvalMode?: ApprovalMode
  /** The SSH destination this conversation's agent runs on for its whole
   * life: one `agents.list` returned under `environments`. Omitted runs it on
   * the gateway's own machine. One the gateway does not name is refused as
   * `environment_not_configured`, with `uncertain` false. */
  environment?: string
}
/** Message admission receipt with the client-owned action identity. */
export type ConversationSubmission = ConversationReceipt & {
  /** Original action identifier. */ requestId: string
}

/** Agent conversation commands. Creation and message admission failures expose NessaConversationMutationError.retry(). Controls expose NessaConversationControlError with uncertain effect status and require a fresh read before another deliberate action. Read returns a bounded full replacement view, suitable for serialized polling. */
export type ConversationApi = {
  /** Create or reopen a conversation. The gateway answers when the original attachment is ready. IDs are generated when options are omitted. */
  create: (options?: ConversationCreateOptions) => Promise<ConversationCreateResult>
  /** Change the provider preset only while this conversation has no running turn. */
  setApprovalMode: (
    conversationId: string,
    mode: ApprovalMode,
    options?: ConversationActionOptions,
  ) => Promise<ConversationSetApprovalModeResult>
  /** Read current provider lifecycle, output, queue, tools, and complete actionable permission choices. */
  read: (conversationId: string) => Promise<ConversationView>
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
  /**
   * Queue a message for this conversation without waiting for provider
   * attachment: text, images, linked files, or any combination of them.
   * @param text - At most 8 KiB UTF-8. May be blank only when the message
   * carries an image or points at a file.
   * @param attachments - The references `client.attachments` returned for
   * images staged into this conversation, in order: at most 10, and 10 MiB
   * together. Omit it, or pass `[]`, for a message that carries no image. A
   * retry re-sends the same list, so the same execution ID always names the
   * same message.
   * @param files - Absolute paths on the machine the gateway runs on, in order,
   * at most 10. Nothing is uploaded for these and nothing is read here: the
   * message names where each file is, and the agent opens it itself if it
   * decides to, which under the gateway's permission policy asks the reader
   * first. A path means nothing to a gateway running anywhere else.
   * @param options - Optional IDs support optimistic UI correlation.
   * @throws TypeError before anything is sent when the message breaks these bounds.
   */
  send: (
    conversationId: string,
    text: string,
    attachments?: readonly ImageAttachment[],
    files?: readonly LinkedFile[],
    options?: ConversationSendOptions,
  ) => Promise<ConversationSubmission>
  /** Submit steering input using the agent's supported steering behavior. Takes the same message as `send`, under the same bounds. */
  steer: (
    conversationId: string,
    text: string,
    attachments?: readonly ImageAttachment[],
    files?: readonly LinkedFile[],
    options?: ConversationSendOptions,
  ) => Promise<ConversationSubmission>
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
  /** Remove the identified waiting input before provider dispatch. */
  remove: (
    conversationId: string,
    executionId: string,
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
    conversationId: string,
    executionId: string,
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
   * Grant one paired device or peer gateway `role` on one of the caller's
   * conversations, or change the role and policy it holds there
   * (`conversation.share`, `credential.manage`). `credentialId` is one
   * `client.credentials.list()` names. A Drive grant needs `policy`; read and
   * comment take none, and the gateway refuses either mistake
   * `invalid_request`, as it does a policy past the schema's bounds (published
   * in `bounds`: `maxToolPolicyPatterns`, `maxToolPatternBytes`,
   * `maxCommandHosts`, `maxCommandHostBytes`). A grant that names no
   * `commandHosts` runs commands nowhere. The answer names the
   * grant's `policyRevision` once the share is made; `applied` is false when
   * the grant already held that role and policy, and its revision was kept.
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
    role: ConversationShareRole,
    policy?: ConversationToolPolicy,
    options?: ConversationShareOptions,
  ) => Promise<ConversationShareResult>
  /**
   * Revoke one paired party's grant on one of the caller's conversations,
   * whatever its role (`conversation.unshare`). It needs only ownership, so it
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
  if (!value.trim() || utf8.encode(value).byteLength > maxBytes)
    throw new TypeError(`${name} must contain 1-${maxBytes} UTF-8 bytes`)
  return value
}
function observeDecimal(value: string): string {
  if (!decimal(value)) throw new TypeError("Invalid observation cursor")
  return value
}
function validConversationId(value: string): string {
  if (!conversationIdPattern.test(value))
    throw new TypeError("Conversation ID must be a canonical lowercase UUID")
  return value
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
  function submit(
    method: string,
    conversationId: string,
    text: string,
    attachments: readonly ImageAttachment[] = [],
    linked: readonly LinkedFile[] = [],
    options: ConversationSendOptions = {},
  ) {
    validConversationId(conversationId)
    const executionId = boundedText(options.executionId ?? newId(), "Execution ID", 256)
    const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
    const problem = imageAttachmentsProblem(attachments)
    if (problem) throw new TypeError(`Invalid message attachments: ${problem}`)
    const filesProblem = linkedFilesProblem(linked)
    if (filesProblem) throw new TypeError(`Invalid message files: ${filesProblem}`)
    // Text may be blank only beside an image or a linked file: the message has
    // to say something.
    if (attachments.length === 0 && linked.length === 0)
      boundedText(text, "Message", 8192)
    else if (typeof text !== "string" || utf8.encode(text).byteLength > 8192)
      throw new TypeError("Message must contain at most 8192 UTF-8 bytes")
    // Copied and frozen with the command, so editing the caller's list between a
    // failure and its retry cannot turn one execution ID into two messages.
    const images = Object.freeze(
      attachments.map(({ digest, mimeType, size }) =>
        Object.freeze({ digest, mimeType, size }),
      ),
    )
    const files = Object.freeze(linked.map(({ path }) => Object.freeze({ path })))
    return mutate(
      method,
      { conversationId, executionId, requestId, text, attachments: images, files },
      (value) => ({
        ...conversationReceipt(value, executionId),
        requestId,
      }),
    )
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
      return mutate(
        ProductMethod.ConversationCreate,
        {
          conversationId: id,
          requestId,
          ...agent,
          ...model,
          ...approvalMode,
          ...environment,
        },
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
    send: (id, text, attachments, files, options) =>
      submit(ProductMethod.ConversationSend, id, text, attachments, files, options),
    steer: (id, text, attachments, files, options) =>
      submit(ProductMethod.ConversationSteer, id, text, attachments, files, options),
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
    remove: (id, executionId, options) =>
      action(ProductMethod.ConversationRemove, id, { executionId }, options),
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
          questionId,
          choices: capturedChoices,
        },
        (value) => conversationMutation(value, requestId),
        false,
      )
    },
    answer: (target, optionId, options) => {
      const { conversationId, executionId, permissionId } = permissionFields(
        target,
        optionId,
      )
      return action(
        ProductMethod.ConversationAnswer,
        conversationId,
        { executionId, permissionId, optionId },
        options,
        true,
      )
    },
    cancel: (target, reason, options) => {
      const { conversationId, executionId, permissionId } = permissionFields(target)
      return action(
        ProductMethod.ConversationCancel,
        conversationId,
        { executionId, permissionId, reason },
        options,
      )
    },
    close: (id, options) => action(ProductMethod.ConversationClose, id, {}, options),
    stop: (id, executionId, options = {}) => {
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
    archive: (id, options) => action(ProductMethod.ConversationArchive, id, {}, options),
    unarchive: (id, options) =>
      action(ProductMethod.ConversationUnarchive, id, {}, options),
    delete: (id, options) =>
      action(ProductMethod.ConversationDelete, id, {}, options, false, {
        atLeastMs: conversationDeleteTimeoutMs,
      }),
    share: (id, credentialId, role, policy, options = {}) => {
      validConversationId(id)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      boundedText(credentialId, "Credential ID", 256)
      // Copied, so a caller changing its policy between a failure and its
      // retry cannot send another grant under the same request ID.
      const frozen =
        policy === undefined
          ? undefined
          : (JSON.parse(JSON.stringify(policy)) as ConversationToolPolicy)
      return mutate(
        ProductMethod.ConversationShare,
        {
          conversationId: id,
          requestId,
          credentialId,
          role,
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
