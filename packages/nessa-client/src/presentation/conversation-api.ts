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
import { conversationDeleteTimeoutMs } from "../application/agent-budgets.js"
import { bounds, ProductMethod } from "../generated/product.js"
import type {
  ConversationCreateResult,
  ConversationListResult,
  ConversationObserveCursor,
  ConversationObserveResult,
  ConversationView,
  ConversationReceipt,
  ConversationMutationResult,
  ConversationQuestionChoice,
  ConversationReorderResult,
  ConversationSetApprovalModeResult,
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
  conversationObserve,
  conversationView,
  conversationReceipt,
  conversationCommandReceipt,
  conversationMutation,
  conversationReorder,
  conversationApprovalMode,
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
/** Optional IDs and delivery mode, filled by the client preparation wrapper. */
export type ConversationPrepareOptions = ConversationActionOptions & {
  /** Reuse only for the same logical message. */ executionId?: string
  /** Queued by default; steering uses the agent's supported behavior. */ mode?:
    "queued" | "steering"
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
export type ConversationSubmission = Readonly<ConversationReceipt> & {
  /** Original conversation identity. */ readonly conversationId: string
  /** Target owned before the admission request started. */ readonly target: ExecutionTarget
  /** Original action identifier. */ readonly requestId: string
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
  /** Fill optional identities and capture a complete message before effects.
   * Text is at most 8 KiB UTF-8 and may be blank only beside an image/file.
   * Images are references returned by attachments staging, not original bytes;
   * files are absolute paths on the gateway's machine. The shared published
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
   * Answer one question the agent asked, or decline it.
   *
   * `choices` of null declines: the agent is told it was asked and answered
   * with nothing, which is an answer rather than silence. A question left out
   * of `choices` is skipped, which every ask permits.
   * A failed acknowledgement is a control error without replay; read the
   * current question before making a new deliberate answer.
   */
  answerQuestion: (
    conversationId: string,
    executionId: string,
    questionId: string,
    choices: readonly ConversationQuestionChoice[] | null,
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
}

const utf8 = new TextEncoder()
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

/** Complete logical message intent before create/admission starts. */
export type SubmissionInput = {
  readonly conversationId: string
  readonly executionId: string
  readonly requestId: string
  readonly mode: "queued" | "steering"
  readonly text: string
  readonly attachments?: readonly ImageAttachment[]
  readonly files?: readonly LinkedFile[]
}
declare const preparedSubmission: unique symbol
/** Process-local immutable message and its captured control target. */
export type PreparedSubmission = Omit<SubmissionInput, "attachments" | "files"> & {
  readonly attachments: readonly Readonly<ImageAttachment>[]
  readonly files: readonly Readonly<LinkedFile>[]
  readonly target: ExecutionTarget
  readonly [preparedSubmission]: true
}
const preparedValues = new WeakSet<PreparedSubmission>()

/** Validate and copy one complete message; no external effect runs here. */
export function prepareSubmission(input: SubmissionInput): PreparedSubmission {
  const { conversationId, text, mode } = input
  const attachments = input.attachments ?? []
  const linked = input.files ?? []
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
  if (images.length === 0 && files.length === 0) boundedText(text, "Message", 8192)
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
      | readonly ConversationQuestionChoice[]
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
    prepare: (conversationId, text, attachments, files, options = {}) =>
      prepareSubmission({
        conversationId,
        text,
        attachments,
        files,
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
        target,
      } = preparedSubmissionFields(prepared)
      return mutate(
        mode === "steering"
          ? ProductMethod.ConversationSteer
          : ProductMethod.ConversationSend,
        { conversationId, executionId, requestId, text, attachments, files },
        (value) =>
          Object.freeze({
            ...conversationReceipt(value, executionId),
            conversationId,
            requestId,
            target,
          }),
      )
    },
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
    answerQuestion: (id, executionId, questionId, choices, options = {}) => {
      validConversationId(id)
      const requestId = boundedText(options.requestId ?? newId(), "Request ID", 256)
      return mutate(
        ProductMethod.ConversationAnswerQuestion,
        {
          conversationId: id,
          requestId,
          executionId,
          questionId,
          choices: choices === null ? null : choices.map((choice) => ({ ...choice })),
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
    archive: (id, options) => action(ProductMethod.ConversationArchive, id, {}, options),
    unarchive: (id, options) =>
      action(ProductMethod.ConversationUnarchive, id, {}, options),
    delete: (id, options) =>
      action(ProductMethod.ConversationDelete, id, {}, options, false, {
        atLeastMs: conversationDeleteTimeoutMs,
      }),
  }
}
