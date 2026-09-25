import {
  NessaConversationMutationError,
  NessaConversationControlError,
} from "../application/conversation-mutation-error.js"
import type { RequestDeadline, RpcRequester } from "../application/session-port.js"
import { conversationDeleteTimeoutMs } from "../application/agent-budgets.js"
import { ProductMethod } from "../generated/product.js"
import type {
  ConversationCreateResult,
  ConversationListResult,
  ConversationView,
  ConversationReceipt,
  ConversationMutationResult,
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
  conversationIdPattern,
  conversationList,
  conversationView,
  conversationReceipt,
  conversationMutation,
  conversationReorder,
} from "../protocol/conversation-validate.js"

/** Which of the caller's conversations `list()` returns. */
export type ConversationListOptions = {
  /** True for archived conversations only; false or omitted for the rest. */
  archived?: boolean
}

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
  /** Create or reopen a conversation and start provider attachment in the background. IDs are generated when options are omitted; read `lifecycle` for attachment progress. */
  create: (options?: ConversationCreateOptions) => Promise<ConversationCreateResult>
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
      return mutate(
        ProductMethod.ConversationCreate,
        { conversationId: id, requestId, ...agent },
        (value) => conversationId(value, id),
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
    archive: (id, options) => action(ProductMethod.ConversationArchive, id, {}, options),
    unarchive: (id, options) =>
      action(ProductMethod.ConversationUnarchive, id, {}, options),
    delete: (id, options) =>
      action(ProductMethod.ConversationDelete, id, {}, options, false, {
        atLeastMs: conversationDeleteTimeoutMs,
      }),
  }
}
