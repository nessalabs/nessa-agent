import {
  NessaConversationMutationError,
  NessaConversationControlError,
} from "../application/conversation-mutation-error.js"
import { agentOperationTimeoutMs } from "../application/agent-budgets.js"
import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod } from "../generated/product.js"
import type {
  ConversationCreateResult,
  ConversationView,
  ConversationReceipt,
  ConversationMutationResult,
  ConversationQuestionChoice,
  ConversationReorderResult,
  ImageAttachment,
  LinkedFile,
} from "../generated/product.js"
import {
  imageAttachmentsProblem,
  linkedFilesProblem,
} from "../protocol/attachment-validate.js"
import {
  conversationId,
  conversationView,
  conversationReceipt,
  conversationMutation,
  conversationReorder,
} from "../protocol/conversation-validate.js"

/** Optional caller-managed action identity. The client generates it when omitted. */
export type ConversationActionOptions = {
  /** Stable action attribution. Only creation and message admission support same-ID retries; controls require a fresh read and a new deliberate action. */
  requestId?: string
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
}
/** Message admission receipt with the client-owned action identity. */
export type ConversationSubmission = ConversationReceipt & {
  /** Original action identifier. */ requestId: string
}

/** Agent conversation commands. Creation and message admission failures expose NessaConversationMutationError.retry(). Controls expose NessaConversationControlError with uncertain effect status and require a fresh read before another deliberate action. Read returns a bounded full replacement view, suitable for serialized polling. */
export type ConversationApi = {
  /** Create or reopen a conversation. IDs are generated when options are omitted. */
  create: (options?: ConversationCreateOptions) => Promise<ConversationCreateResult>
  /** Read current output, queue, tools, and complete actionable permission choices. */
  read: (conversationId: string) => Promise<ConversationView>
  /**
   * Queue a message for this conversation: text, images, linked files, or any
   * combination of them.
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
    conversationId: string,
    executionId: string,
    permissionId: string,
    optionId: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /**
   * Answer one question the agent asked, or decline it.
   *
   * `choices` of null declines: the agent is told it was asked and answered
   * with nothing, which is an answer rather than silence. A question left out
   * of `choices` is skipped, which every ask permits.
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
    conversationId: string,
    executionId: string,
    permissionId: string,
    reason: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
  /** Close the provider context and stop admitted work; saved conversation history remains. */
  close: (
    conversationId: string,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
}

const utf8 = new TextEncoder()
const conversationIdPattern =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
function boundedText(value: string, name: string, maxBytes: number): string {
  if (!value.trim() || utf8.encode(value).byteLength > maxBytes)
    throw new TypeError(`${name} must contain 1-${maxBytes} UTF-8 bytes`)
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
      | readonly ConversationQuestionChoice[]
      | null
      | undefined
    >,
    validate: (value: unknown) => T,
    retryable = true,
    permissionAnswer = false,
  ): Promise<T> {
    const command = Object.freeze({ ...params })
    const perform = async (): Promise<T> => {
      try {
        return validate(
          await session.request(method, command, {
            atLeastMs: agentOperationTimeoutMs,
          }),
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
      return mutate(
        ProductMethod.ConversationCreate,
        { conversationId: id, requestId, ...agent },
        (value) => conversationId(value, id),
      )
    },
    read: async (id) =>
      conversationView(
        await session.request(
          ProductMethod.ConversationRead,
          { conversationId: validConversationId(id) },
          { atLeastMs: agentOperationTimeoutMs },
        ),
        id,
      ),
    send: (id, text, attachments, files, options) =>
      submit(ProductMethod.ConversationSend, id, text, attachments, files, options),
    steer: (id, text, attachments, files, options) =>
      submit(ProductMethod.ConversationSteer, id, text, attachments, files, options),
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
      )
    },
    answer: (id, executionId, permissionId, optionId, options) =>
      action(
        ProductMethod.ConversationAnswer,
        id,
        { executionId, permissionId, optionId },
        options,
        true,
      ),
    cancel: (id, executionId, permissionId, reason, options) =>
      action(
        ProductMethod.ConversationCancel,
        id,
        { executionId, permissionId, reason },
        options,
      ),
    close: (id, options) => action(ProductMethod.ConversationClose, id, {}, options),
  }
}
