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
    error?: string
    status: string
  }[]
  pending: { executionId: string; text: string; mode: "queued" | "steering" }[]
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
  capabilities: { queue: boolean; steer: boolean; resume: boolean; permissions: boolean }
  permissionViewError?: string
}

/** Stable logical identities survive an uncertain acknowledgement and explicit retry. */
export type Submission = {
  runtime?: { model: string; provider: string; workspace: string }
  conversationId: string
  executionId: string
  actionId: string
  text: string
}
export type SubmissionReceipt = { executionId: string; disposition: string }
