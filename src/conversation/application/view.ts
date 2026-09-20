import type { ImageReference } from "../model"
import type { AgentPart } from "../model/types"
/** Authorized bounded gateway projection. It does not own execution scheduling. */
export type ConversationView = {
  runtime?: { model: string; provider: string; workspace: string }
  conversationId: string
  revision: string
  queueComplete: boolean
  truncated: boolean
  messages: {
    executionId: string
    steeringTarget?: string
    steeringOffset?: number
    parts: AgentPart[]
    userText: string
    /** Images sent with this turn, by reference. The view never carries bytes. */
    attachments: ImageReference[]
    error?: string
    status: string
  }[]
  pending: {
    executionId: string
    text: string
    attachments: ImageReference[]
    mode: "queued" | "steering"
  }[]
  permissions: {
    executionId: string
    permissionId: string
    toolName: string
    argumentsJson: string
    toolId: string
    title: string
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
  capabilities: {
    queue: boolean
    steer: boolean
    resume: boolean
    permissions: boolean
    /** The opened agent takes images. False until one is open, so never assumed. */
    imageInput: boolean
  }
  permissionViewError?: string
}

/** Stable logical identities survive an uncertain acknowledgement and explicit retry. */
export type Submission = {
  runtime?: { model: string; provider: string; workspace: string }
  conversationId: string
  executionId: string
  actionId: string
  /** May be blank only when `attachments` is not empty. */
  text: string
  /** Images already staged into this conversation. A retry re-sends the same list. */
  attachments: ImageReference[]
}
export type SubmissionReceipt = { executionId: string; disposition: string }
