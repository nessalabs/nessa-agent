import type { ConversationErrorCode } from "@nessa/client"
import type { ImageReference } from "./attachments"
import type { MessageContent } from "./content"

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
  /** Typed gateway rejection behind `error`, when the failure carried one. Notices branch on this, never on the message text. */
  errorCode?: ConversationErrorCode
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
