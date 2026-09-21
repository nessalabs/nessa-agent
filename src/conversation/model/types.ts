import type { ImageReference } from "./attachments"
import type { MessageContent } from "./content"

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
  | "invalid-request"

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
  kind: "text" | "thought" | "tool"
  text: string
  toolId: string
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
  readError?: string
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
    tools: {
      executionId: string
      toolId: string
      title: string
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
    capabilities: {
      queue: boolean
      steer: boolean
      resume: boolean
      permissions: boolean
      imageInput: boolean
    }
    queueComplete: boolean
    truncated: boolean
    permissionViewError?: string
  }
}
export type IdleConversation = ConversationState & { phase: "idle" }
export type BusyConversation = ConversationState & {
  phase: "thinking" | "streaming"
  pending: string
}

export type Conversation = IdleConversation | BusyConversation

export type Phase = Conversation["phase"]

export function conversation(id: string): IdleConversation {
  return { id, title: "New chat", turns: [], phase: "idle", draft: [] }
}
