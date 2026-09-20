import { NessaRpcError } from "./rpc-error.js"
import type { ConversationPermissionSelectionState } from "../generated/product.js"

function permissionSelection(
  cause: unknown,
): ConversationPermissionSelectionState | undefined {
  if (
    !(cause instanceof NessaRpcError) ||
    !cause.details ||
    typeof cause.details !== "object" ||
    Array.isArray(cause.details) ||
    Object.keys(cause.details).length !== 1
  )
    return undefined
  const value = (cause.details as Record<string, unknown>).selectionState
  return value === "pending" || value === "consumed" || value === "unknown"
    ? value
    : undefined
}

/**
 * The gateway's refusals that are decided before a command is admitted, so that
 * receiving one as the RPC's answer proves nothing was queued or dispatched.
 *
 * Closed on purpose, and each entry checked against the gateway: in `submit`
 * these are all raised before `enqueue` returns a receipt. A code that is not
 * here — `temporarily_unavailable` among them, which a supervising task also
 * reports when work it had already admitted was lost — leaves the outcome
 * uncertain, which is the safe reading.
 */
export type ConversationRejection =
  /** Malformed or out-of-bounds command. */
  | "invalid_request"
  /** The gateway started with no agent to send to. */
  | "agent_not_configured"
  /** The conversation's agent does not take images. */
  | "image_input_unsupported"
  /** A named image is not held by this conversation: never uploaded into it, expired, or released when it closed. */
  | "attachment_not_found"
  /** A named image is held but could not be read for the agent. */
  | "attachment_unavailable"
  /** No such conversation for this caller. */
  | "conversation_not_found"
  /** The gateway is at its limit of open conversations. */
  | "conversation_capacity"

const rejections: readonly ConversationRejection[] = [
  "invalid_request",
  "agent_not_configured",
  "image_input_unsupported",
  "attachment_not_found",
  "attachment_unavailable",
  "conversation_not_found",
  "conversation_capacity",
]

function rejection(cause: unknown): ConversationRejection | undefined {
  return cause instanceof NessaRpcError
    ? rejections.find((known) => known === cause.code)
    : undefined
}

/** Failed conversation creation or message admission with its original identities and a safe same-command retry. No request is replayed automatically. */
export class NessaConversationMutationError<T> extends Error {
  /** False only when the gateway explicitly rejected the command before admission. */
  readonly uncertain: boolean
  /** Which pre-admission refusal this was; undefined exactly when `uncertain` is true. Branch on this, never on the message. */
  readonly rejection: ConversationRejection | undefined

  constructor(
    /** Conversation whose command failed. */
    readonly conversationId: string,
    /** Stable action ID retained across retries. */
    readonly requestId: string,
    /** Stable message ID when this command targets an invocation. */
    readonly executionId: string | undefined,
    cause: unknown,
    private readonly repeat: () => Promise<T>,
  ) {
    super(
      cause instanceof NessaRpcError && cause.code === "agent_not_configured"
        ? 'The gateway started without an agent: its config.json has no "agent" section, so there is no Claude ACP runtime to send to. Configure one and restart the gateway — from a Nessa checkout, `just server` writes one.'
        : "Conversation command failed",
      { cause },
    )
    this.rejection = rejection(cause)
    this.uncertain = this.rejection === undefined
    this.name = "NessaConversationMutationError"
  }

  /** Retry the original immutable command after reconnecting or addressing its cause. Retains both action and execution IDs. */
  retry(): Promise<T> {
    return this.repeat()
  }
}

/** A conversation control failed. Read current state before deliberately choosing another action; replaying close could stop newer work. Permission answers retain the gateway's separate selection-state evidence. */
export class NessaConversationControlError extends Error {
  /** False when the gateway explicitly rejected the command before applying it. */
  readonly uncertain: boolean
  /** Permission-answer state reported by the gateway. Undefined for other controls or failures without trustworthy typed details. */
  readonly permissionSelection: ConversationPermissionSelectionState | undefined

  constructor(
    /** Conversation targeted by the control. */
    readonly conversationId: string,
    /** Original action identity, retained for diagnosis rather than replay. */
    readonly requestId: string,
    /** Invocation targeted by the control, if any. */
    readonly executionId: string | undefined,
    cause: unknown,
    /** Whether this command is a permission answer whose validated error details carry selection state. @internal */
    permissionAnswer = false,
  ) {
    super("Conversation control did not return a trustworthy acknowledgement", { cause })
    this.permissionSelection = permissionAnswer ? permissionSelection(cause) : undefined
    this.uncertain =
      this.permissionSelection !== "pending" &&
      !(cause instanceof NessaRpcError && cause.code === "invalid_request")
    this.name = "NessaConversationControlError"
  }
}
