import { NessaRpcError } from "./rpc-error.js"
import {
  ConversationErrorCode,
  type ConversationPermissionSelectionState,
} from "../generated/product.js"

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
 * The conversation rejection code this build knows by that name, or `undefined`
 * for any other string.
 *
 * A gateway that returns a code this build does not know is not reinterpreted as
 * one it does: the caller gets no typed code and keeps the original cause, which
 * is what an answer nobody here has a meaning for deserves. Use it wherever a
 * raw wire code has to be narrowed before it can be branched on — the errors
 * below do it for commands, and `conversation.read` rejects with the underlying
 * {@link NessaRpcError}, whose `code` is a plain string until this narrows it.
 *
 * @param code - The `code` of a `type: "res"` error frame, as it arrived.
 * @returns The matching {@link ConversationErrorCode}, or `undefined`.
 */
export const conversationErrorCode = (code: string): ConversationErrorCode | undefined =>
  (Object.values(ConversationErrorCode) as string[]).includes(code)
    ? (code as ConversationErrorCode)
    : undefined

// Codes the gateway only returns after refusing the command outright. A startup
// deadline belongs here: startup ends before any input reaches the provider.
// Closed on purpose, and each entry checked against the gateway: in `submit`
// the image codes are all raised before `enqueue` returns a receipt. A code
// that is not here — `temporarily_unavailable` among them, which a supervising
// task also reports when work it had already admitted was lost — leaves the
// outcome uncertain, which is the safe reading.
const rejectedBeforeAdmission = (code: string): boolean =>
  (
    [
      ConversationErrorCode.AgentNotConfigured,
      ConversationErrorCode.InvalidRequest,
      ConversationErrorCode.AgentStartupDeadline,
      ConversationErrorCode.ConversationNotFound,
      ConversationErrorCode.ConversationCapacity,
      ConversationErrorCode.ImageInputUnsupported,
      ConversationErrorCode.AttachmentNotFound,
      ConversationErrorCode.AttachmentUnavailable,
    ] as string[]
  ).includes(code)

// A control has no input to admit, so the question is only whether it could
// have been applied. Startup ends before any control reaches the provider.
const rejectedBeforeApplying = (code: string): boolean =>
  (
    [
      ConversationErrorCode.InvalidRequest,
      ConversationErrorCode.AgentStartupDeadline,
    ] as string[]
  ).includes(code)

function rejectionMessage(cause: unknown): string {
  if (!(cause instanceof NessaRpcError)) return "Conversation command failed"
  switch (cause.code) {
    case ConversationErrorCode.AgentNotConfigured:
      return 'The gateway started without an agent: its config.json has no "agent" section, so there is no Claude ACP runtime to send to. Configure one and restart the gateway — from a Nessa checkout, `just server` writes one.'
    // Not a guarantee: a launch whose process could not be confirmed stopped
    // keeps its conversation blocked, and the gateway cannot tell the two apart
    // in this code. Retry is still the right next step, and normally succeeds.
    case ConversationErrorCode.AgentStartupDeadline:
      return "The agent was still starting and ran out of time, so nothing was sent. Starting it is slowest the first time after an install or update, while the operating system scans the runtime. Retry normally succeeds once the runtime is warm."
    default:
      return "Conversation command failed"
  }
}

/** Failed conversation creation or message admission with its original identities and a safe same-command retry. No request is replayed automatically. */
export class NessaConversationMutationError<T> extends Error {
  /** False only when the gateway explicitly rejected the command before admission. A rejected create may still have recorded the conversation's ownership; creation is idempotent by conversation ID, so repeating it is safe. */
  readonly uncertain: boolean
  /** Typed conversation rejection code. Undefined for a transport failure, and also for a gateway rejection this build does not know — including the access and routing codes the socket answers with before a conversation command is dispatched. Inspect `cause` for those. Branch on this rather than on the message. */
  readonly code: ConversationErrorCode | undefined

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
    super(rejectionMessage(cause), { cause })
    this.code =
      cause instanceof NessaRpcError ? conversationErrorCode(cause.code) : undefined
    this.uncertain = !(
      cause instanceof NessaRpcError && rejectedBeforeAdmission(cause.code)
    )
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
  /** Typed conversation rejection code, with the same meaning and the same limits as NessaConversationMutationError.code. */
  readonly code: ConversationErrorCode | undefined
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
    this.code =
      cause instanceof NessaRpcError ? conversationErrorCode(cause.code) : undefined
    this.uncertain =
      this.permissionSelection !== "pending" &&
      !(cause instanceof NessaRpcError && rejectedBeforeApplying(cause.code))
    this.name = "NessaConversationControlError"
  }
}
