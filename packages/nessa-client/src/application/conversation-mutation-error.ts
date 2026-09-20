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

/** Refusals this gateway states outright, and what each one means to a person.
 *
 * Kept apart from one another rather than sharing a code: "this gateway runs no
 * conversations", "it is not set up for the agent you asked for" and "no build
 * here can open that conversation at all" are three different situations, and
 * only one of them is fixed by configuring anything. A caller told the wrong one
 * goes and changes something that was never the problem. */
const REFUSALS: Partial<Record<ConversationErrorCode, string>> = {
  // Keyed by the wire literal rather than by `ConversationErrorCode.X`, so the
  // gateway's own seam test can read this file and see each code it can send.
  conversations_not_configured: "This gateway is not set up to run conversations.",
  agent_not_configured:
    'This gateway is not set up for the agent this conversation asked for: its config.json names no runtime under that name. Add one under "agents.runtimes" and restart the gateway — from a Nessa checkout, `just server` writes one.',
  agent_unsupported:
    "This conversation runs on an agent this version of Nessa cannot open.",
  // Not a guarantee: a launch whose process could not be confirmed stopped
  // keeps its conversation blocked, and the gateway cannot tell the two apart
  // in this code. Retry is still the right next step, and normally succeeds.
  agent_startup_deadline:
    "The agent was still starting and ran out of time, so nothing was sent. Starting it is slowest the first time after an install or update, while the operating system scans the runtime. Retry normally succeeds once the runtime is warm.",
}

/**
 * What this gateway said, if it said one of these and not something inherited.
 *
 * The code is unvalidated wire text, so it is turned into a known code before
 * it is used as a key: `code in REFUSALS` and `REFUSALS[code]` walk the
 * prototype chain, and a frame answering `toString` or `__proto__` was read as
 * a refusal this gateway had stated — which made the failure certain when the
 * client knew nothing of the kind, and put a native function where the
 * explanation belongs.
 */
function refusal(cause: unknown): string | undefined {
  if (!(cause instanceof NessaRpcError)) return undefined
  const code = knownCode(cause.code)
  return code === undefined ? undefined : REFUSALS[code]
}

/** Codes the gateway only returns after refusing the command outright.
 *
 * One list for both classes, because both questions have the same answer. A
 * creation asks whether its input could already have been admitted; a control
 * asks whether it could already have been applied — and a control resolves its
 * conversation before it is dispatched, so it meets exactly the refusals a
 * creation meets and meets them just as early. A startup deadline belongs here
 * too: startup ends before anything reaches the provider. */
const rejectedBeforeDispatch = (code: string): boolean =>
  (
    [
      ConversationErrorCode.AgentNotConfigured,
      ConversationErrorCode.AgentUnsupported,
      ConversationErrorCode.ConversationsNotConfigured,
      ConversationErrorCode.InvalidRequest,
      ConversationErrorCode.AgentStartupDeadline,
    ] as string[]
  ).includes(code)

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
    super(refusal(cause) ?? "Conversation command failed", { cause })
    this.code = cause instanceof NessaRpcError ? knownCode(cause.code) : undefined
    // A refusal is a decision this gateway has already made, so the command
    // never reached an agent and nothing about it is in doubt. Everything else
    // may have been admitted before the failure and is reported as uncertain.
    this.uncertain = !(
      cause instanceof NessaRpcError && rejectedBeforeDispatch(cause.code)
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
    this.code = cause instanceof NessaRpcError ? knownCode(cause.code) : undefined
    // Still only `pending` and an outright refusal make a control certain. A
    // refusal is stated before the control is applied, which is why it counts;
    // anything else may have been applied already, and replaying a close is
    // what that warns about.
    this.uncertain =
      this.permissionSelection !== "pending" &&
      !(cause instanceof NessaRpcError && rejectedBeforeDispatch(cause.code))
    this.name = "NessaConversationControlError"
  }
}
