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

/** Failed conversation creation or message admission with its original identities and a safe same-command retry. No request is replayed automatically. */
/** Refusals this gateway states outright, and what each one means to a person.
 *
 * Kept apart from one another rather than sharing a code: "this gateway runs no
 * conversations", "it is not set up for the agent you asked for" and "no build
 * here can open that conversation at all" are three different situations, and
 * only one of them is fixed by configuring anything. A caller told the wrong one
 * goes and changes something that was never the problem. */
const REFUSALS: Record<string, string> = {
  conversations_not_configured: "This gateway is not set up to run conversations.",
  agent_not_configured:
    "This gateway is not set up for the agent this conversation asked for.",
  agent_unsupported:
    "This conversation runs on an agent this version of Nessa cannot open.",
}

export class NessaConversationMutationError<T> extends Error {
  /** False only when the gateway explicitly rejected the command before admission. */
  readonly uncertain: boolean

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
      (cause instanceof NessaRpcError ? REFUSALS[cause.code] : undefined) ??
        "Conversation command failed",
      { cause },
    )
    // A refusal is a decision this gateway has already made, so the command
    // never reached an agent and nothing about it is in doubt. Everything else
    // may have been admitted before the failure and is reported as uncertain.
    this.uncertain = !(
      cause instanceof NessaRpcError &&
      (cause.code in REFUSALS || cause.code === "invalid_request")
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
