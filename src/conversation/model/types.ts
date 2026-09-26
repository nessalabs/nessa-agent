import type { ImageReference } from "./attachments"
import type { MessageContent } from "./content"

export type AgentFeatures = {
  permissionDenial: "unknown" | "unsupported" | "supported_for_offered_permission_reviews"
  nativeHookSuppression: "unknown" | "unsupported" | "supported_for_user_configured_hooks"
  compactionReporting:
    "unsupported_not_implemented" | "supported_with_invocation_correlation"
  modelSwitchReporting: "unsupported_not_implemented" | "supported_after_validated_switch"
  permissionDeferral: "unsupported_not_implemented" | "supported_with_nonterminal_outcome"
  elicitationForwarding:
    "unknown" | "unsupported" | "supported_with_correlated_round_trip"
  preToolPolicy:
    "unknown" | "unsupported_not_implemented" | "supported_at_permission_gate"
  policyEndTurn:
    "unknown" | "unsupported_not_implemented" | "supported_for_current_invocation"
  policyCloseSession: "unknown" | "unsupported_not_implemented" | "supported_for_session"
  incomingElicitation:
    "unknown" | "unsupported_not_implemented" | "supported_with_correlated_round_trip"
}

export type ConversationCapabilities = {
  queue: boolean
  steer: boolean
  resume: boolean
  permissions: boolean
  imageInput: boolean
  agentFeatures: AgentFeatures
}

/**
 * Why a conversation command did not do what was asked, in this panel's own
 * words rather than the gateway's.
 *
 * The gateway answers in wire codes. `adapters/gateway/effects.ts` is the one
 * place those are read, and these are what everything after it says about a
 * failure: which sentence it shows, and which heading.
 *
 * Why, and never whether. A reason says nothing about whether the command ran,
 * and the same reason means different things for different commands: a message
 * refused as `conversation-not-found` was certainly not sent, while a control
 * that met the same code may still have been applied — the gateway can lose
 * the acknowledgement rather than the command. What happened is a separate
 * typed fact, carried by the error that names it: `SubmissionRefusedError`
 * exists only for a message that was not taken, and `ControlFailedError`
 * carries its own `refused`. Decide from those; use this to speak.
 */
export type CommandFailure =
  | "image-input-unsupported"
  | "attachment-not-found"
  | "attachment-unavailable"
  | "attachment-cleanup-unavailable"
  | "conversation-not-found"
  | "conversation-capacity"
  | "agent-not-configured"
  | "agent-unsupported"
  | "conversations-not-configured"
  | "agent-startup-deadline"
  | "conversation-state-unreadable"
  | "invalid-request"
  /**
   * The command never left this window: there was no gateway connection to
   * send it on. Certain, and nothing was done.
   */
  | "not-connected"
  /** Somebody deleted the conversation; its identity is refused from now on. */
  | "conversation-deleted"
  /**
   * A delete that happened — the conversation is gone and refused — whose
   * erasure of stored data did not finish; the gateway tries again, at the
   * latest when it next starts. News
   * about a delete, like `attachment-cleanup-unavailable` is about a close.
   */
  | "conversation-erasure-incomplete"
  /**
   * A delete that happened whose record of it could not be finished — the
   * deletion record refused, or only the uploads' own evidence lost. The
   * gateway answers a delete with `audit_unavailable` only once it is deleted.
   */
  | "deletion-unrecorded"

/**
 * Why the panel could not refresh a conversation's view, in this panel's own
 * words rather than the gateway's.
 *
 * A sibling of {@link CommandFailure} and not a member of it, because a read is
 * not a command: nothing was asked for and nothing was changed, so there is no
 * receipt, no draft to hand back, and no outcome to be certain or uncertain
 * about. What a read failure decides is narrower — which sentence appears under
 * a transcript that has stopped moving, and whether a retry is worth offering —
 * and it answers for codes a command never meets
 * (`conversation_configuration_changed`) while having nothing to say about most
 * of the ones a command does. Folding the two together would make every switch
 * over a command's reasons answer for a read's, and the other way round.
 *
 * Two words, because the panel keeps polling and keeps showing the last view
 * whatever went wrong, so a word earns its place only by changing what is said
 * or whether a retry is offered:
 *
 * - `configuration-changed` is the one the gateway will not serve again. The
 *   conversation was created against an agent configuration it no longer has,
 *   which nothing this window does can change, so it withdraws the retry and
 *   outranks anything else the tab is saying.
 * - `unavailable` is every other failed read, and claims nothing beyond a stale
 *   view and a panel still asking.
 *
 * A third word for a *transient* failure was written and then withdrawn, because
 * no code the gateway sends means that. `temporarily_unavailable` and
 * `agent_startup_deadline` look like "not yet", and usually are — but when a
 * failed launch cannot be confirmed stopped, `ConversationService` deliberately
 * retains the conversation's slot (`retryable` is false at
 * `conversation/application/service.rs`, and the slot is evicted only when it is
 * true), so every later read is answered from the cached failure and the
 * provider is never attempted again. Its own test asserts exactly that:
 * `a_startup_deadline_with_unconfirmed_cleanup_retains_its_slot`. The protocol
 * text this panel's client is generated from says it too — "a launch whose
 * process could not be confirmed stopped keeps that conversation blocked" — and
 * an adapter panic during opening takes the same road to
 * `temporarily_unavailable`. The gateway cannot tell the two apart in the code
 * it sends, so neither can the panel, and promising that a conversation which
 * may be blocked until the gateway restarts "catches up in a moment" would be a
 * confident falsehood where the honest sentence costs nothing: `unavailable`
 * already says the panel keeps trying, which is true of all of them.
 *
 * `conversation_not_found` and `agent_not_configured` were weighed too, and left
 * with the rest: neither is permanent — a re-authenticated session or a
 * reconfigured gateway makes both succeed — and neither changes what somebody
 * does next. `unavailable` is also the word a code this build has never heard of
 * must get, and a sentence good enough for an unknown answer is good enough for
 * a known one nobody can act on.
 */
export type ReadFailure =
  | "configuration-changed"
  /**
   * The gateway cannot read this conversation's saved state, and will not be
   * able to later: it caches that failure and answers every later read from it
   * without going near the provider again. Distinct from `unavailable`
   * precisely because that one promises the panel keeps trying, and here there
   * is nothing to keep trying with.
   */
  | "state-unreadable"
  /**
   * Somebody deleted the conversation, here or on another surface. Like
   * `state-unreadable` there is nothing to keep trying: the gateway refuses the
   * identity for good.
   */
  | "deleted"
  | "unavailable"

export type Receipt =
  "sending" | "accepted" | "queued" | "unknown" | "failed" | "delivered"

export type UserTurn = {
  id: string
  from: "user"
  content: MessageContent
  receipt: Receipt
  executionId?: string
  steeringTarget?: string
  steeringOffset?: number
  actionId?: string
  mode?: "queued" | "steering"
  error?: string
}

export type AgentPart = {
  messageId?: string
  offset: number
  kind: "text" | "thought" | "tool" | "local_notice"
  text: string
  toolId: string
  noticeId: string
}

export type AssistantTurn = {
  parts: AgentPart[]
  executionId?: string
  id: string
  from: "assistant"
  text: string
  status?: string
  thought?: string
}

export type Turn = UserTurn | AssistantTurn

type ConversationState = {
  id: string
  title: string
  titleEdited?: boolean
  turns: Turn[]
  draft: MessageContent
  /** Increments only when a failed send restores the draft, resetting the uncontrolled editor. */
  draftReset?: number
  /** Stable gateway identity; local tab closure does not close shared work. */
  serverConversationId?: string
  serverReady?: boolean
  error?: string
  /**
   * Typed reason behind `error`, when the failure carried one. It is what a
   * notice says the failure was, instead of reading the message text. It is not
   * what happened: the turn's own `receipt` says whether a message went, and
   * a control's outcome was decided before this was stored.
   */
  failure?: CommandFailure
  /**
   * Why the last read of this conversation failed, in the panel's own words.
   * Set only while the view on screen is older than the gateway's: any view
   * that arrives clears it, and it says nothing about `error`, which belongs to
   * a command somebody asked for rather than to the panel's own polling.
   */
  readError?: ReadFailure
  revision?: string
  readRequest?: string
  cancellationStatus?: "cancelling" | "cancelled"
  controlPending?: boolean
  remote?: {
    runtime?: { model: string; provider: string; workspace: string }
    /** Last gateway view reports an invocation still running, even before its first chunk. */
    running: boolean
    permissions: {
      executionId: string
      permissionId: string
      toolId: string
      title: string
      toolName: string
      argumentsJson: string
      options: { id: string; label: string }[]
    }[]
    /** Questions the agent is waiting on. Nothing is authorised by answering. */
    questions: {
      executionId: string
      questionId: string
      message: string
      questions: {
        key: string
        prompt: string
        header?: string | null
        multiSelect: boolean
        freeText: boolean
        required: boolean
        options: { value: string; label: string; description?: string | null }[]
      }[]
    }[]
    tools: {
      executionId: string
      toolId: string
      title: string
      /** What the call does, as the provider categorised it; empty until it says. */
      kind: string
      status: string
      input: string
      details: string
    }[]
    pending: {
      executionId: string
      text: string
      attachments: ImageReference[]
      mode: "queued" | "steering"
    }[]
    capabilities: ConversationCapabilities
    lifecycle: {
      phase: "absent" | "starting" | "attached" | "failed"
      failure?: {
        code: "audit" | "provider" | "storage" | "cleanup"
        message: string
      }
      evidenceFailure?: {
        code: "audit"
        message: string
      }
    }
    queueComplete: boolean
    truncated: boolean
    permissionViewError?: string
  }
}
export type IdleConversation = ConversationState & { phase: "idle" }
export type BusyConversation = ConversationState & {
  phase: "starting" | "thinking" | "streaming"
  pending: string
}

export type Conversation = IdleConversation | BusyConversation

export type Phase = Conversation["phase"]

export function conversation(id: string): IdleConversation {
  return { id, title: "New chat", turns: [], phase: "idle", draft: [] }
}
