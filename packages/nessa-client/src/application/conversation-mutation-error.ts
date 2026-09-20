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

// A gateway that returns a code this build does not know is not reinterpreted
// as one it does; the caller sees no typed code and keeps the original cause.
const knownCode = (code: string): ConversationErrorCode | undefined =>
  (Object.values(ConversationErrorCode) as string[]).includes(code)
    ? (code as ConversationErrorCode)
    : undefined

// Codes the gateway only returns after refusing the command outright. A startup
// deadline belongs here: startup ends before any input reaches the provider.
const rejectedBeforeAdmission = (code: string): boolean =>
  (
    [
      ConversationErrorCode.AgentNotConfigured,
      ConversationErrorCode.InvalidRequest,
      ConversationErrorCode.AgentStartupDeadline,
    ] as string[]
  ).includes(code)

function rejectionMessage(cause: unknown): string {
  if (!(cause instanceof NessaRpcError)) return "Conversation command failed"
  switch (cause.code) {
    case ConversationErrorCode.AgentNotConfigured:
      return 'The gateway started without an agent: its config.json has no "agent" section, so there is no Claude ACP runtime to send to. Configure one and restart the gateway — from a Nessa checkout, `just server` writes one.'
    case ConversationErrorCode.AgentStartupDeadline:
      return "The agent was still starting and ran out of time, so nothing was sent. Starting it is slowest the first time after an install or update, while the operating system scans the runtime. Retrying is expected to work."
    default:
      return "Conversation command failed"
  }
}

/** Failed conversation creation or message admission with its original identities and a safe same-command retry. No request is replayed automatically. */
export class NessaConversationMutationError<T> extends Error {
  /** False only when the gateway explicitly rejected the command before admission. */
  readonly uncertain: boolean
  /** Typed gateway rejection code, or undefined when the command failed in transport. Branch on this rather than on the message. */
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
    this.code = cause instanceof NessaRpcError ? knownCode(cause.code) : undefined
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
