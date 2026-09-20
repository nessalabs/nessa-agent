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
    'This gateway is not set up for the agent this conversation asked for: its config.json names no runtime under that name. Add one under "agents.runtimes" and restart the gateway — from a Nessa checkout, `just server` writes one.',
  agent_unsupported:
    "This conversation runs on an agent this version of Nessa cannot open.",
}

/**
 * What this gateway said, if it said one of these and not something inherited.
 *
 * `code in REFUSALS` and `REFUSALS[code]` walk the prototype chain, and the
 * code is unvalidated wire text — a frame answering `toString` or `__proto__`
 * was read as a refusal this gateway had stated, which made the failure
 * certain when the client knew nothing of the kind, and put a native function
 * where the explanation belongs. This library's job is to treat a frame as
 * untrusted, and this is the one place a wire string is used as a key.
 */
function refusal(cause: unknown): string | undefined {
  if (!(cause instanceof NessaRpcError)) return undefined
  return Object.hasOwn(REFUSALS, cause.code) ? REFUSALS[cause.code] : undefined
}

/** Whether this gateway refused the command outright rather than failing it. */
function refused(cause: unknown): boolean {
  return (
    cause instanceof NessaRpcError &&
    (refusal(cause) !== undefined || cause.code === "invalid_request")
  )
}

/** Failed conversation creation or message admission with its original identities and a safe same-command retry. No request is replayed automatically. */
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
    super(refusal(cause) ?? "Conversation command failed", { cause })
    // A refusal is a decision this gateway has already made, so the command
    // never reached an agent and nothing about it is in doubt. Everything else
    // may have been admitted before the failure and is reported as uncertain.
    this.uncertain = !refused(cause)
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
    // The same refusal, said the same way. `close`, `remove`, `answer`,
    // `cancel` and `reorder` all resolve the conversation before the control is
    // dispatched, so they meet exactly the refusals a creation meets — an agent
    // dropped from `config.json`, a record naming an agent this build cannot
    // open — and used to report them as an unknown outcome with no reason
    // given, in the one situation where the reason is the whole fix.
    super(
      refusal(cause) ??
        "Conversation control did not return a trustworthy acknowledgement",
      { cause },
    )
    this.permissionSelection = permissionAnswer ? permissionSelection(cause) : undefined
    // Still only `pending` and an outright refusal make a control certain. A
    // refusal is stated before the control is applied, which is why it counts;
    // anything else may have been applied already, and replaying a close is
    // what that warns about.
    this.uncertain = this.permissionSelection !== "pending" && !refused(cause)
    this.name = "NessaConversationControlError"
  }
}
