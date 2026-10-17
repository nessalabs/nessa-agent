import type {
  ApprovalMode,
  ApprovalModeChoice,
  ConversationCapabilities,
  ConversationLease,
  ConversationRuntime,
  ConversationTranscriptState,
  ImageReference,
  LinkedFile,
  Note,
  PublishedFile,
} from "../model"
import type { AgentPart } from "../model/types"
/** Authorized bounded gateway projection. It does not own execution scheduling. */
export type ConversationView = {
  runtime?: ConversationRuntime
  /** The latest lease, absent before any was recorded. */
  lease?: ConversationLease
  /**
   * Files its agent published, the newest 64 in the order they were recorded.
   * A record is history: it stays after the bytes are let go of.
   */
  artifacts: PublishedFile[]
  /**
   * Notes left in the conversation, the newest 64 in the order they were
   * left: who left each, where it stands, and the turn it follows.
   */
  notes: Note[]
  /**
   * Records this read could not apply, oldest first. Each is one line in the
   * transcript. Empty when every record applied.
   */
  unreadable: { id: string; afterExecutionId?: string }[]
  /**
   * More records could not be read than `unreadable` lists. The transcript
   * ends with one line for that.
   */
  unreadableTruncated: boolean
  approvalMode: ApprovalMode
  approvalModes: ApprovalModeChoice[]
  approvalModeChange?: {
    requestId: string
    requestedMode: ApprovalMode
    status: string
  }
  /**
   * What the gateway calls the conversation — the one title rule, the one the
   * Messages list shows — or null before anything was said in it.
   */
  title: string | null
  conversationId: string
  revision: string
  queueComplete: boolean
  /** Physical committed history status, separate from the bounded display window. */
  transcriptState: ConversationTranscriptState
  truncated: boolean
  messages: {
    executionId: string
    steeringTarget?: string
    steeringOffset?: number
    parts: AgentPart[]
    userText: string
    /** Images sent with this turn, by reference. The view never carries bytes. */
    attachments: ImageReference[]
    /** Files this turn pointed the agent at, by path. */
    files: LinkedFile[]
    authenticationRequired?: boolean
    /**
     * Independent required work failed on this authentication refusal.
     * The person-facing diagnostic stays in `error`.
     */
    requiredWorkFailed?: boolean
    error?: string
    status: string
  }[]
  pending: {
    executionId: string
    text: string
    attachments: ImageReference[]
    files: LinkedFile[]
    mode: "queued" | "steering"
  }[]
  /**
   * Reviews the view lists whose content has not been read yet: no answer
   * is offered on one until it has. `failed` while a read is being retried.
   */
  reviewsWaiting: { review: string; toolName: string; failed: boolean }[]
  /** Waiting reviews, each read whole from its live owner. */
  permissions: {
    /** The key the view lists it under; an answer names it. */
    review: string
    executionId: string
    permissionId: string
    toolName: string
    /** The whole input, read from the review\'s live owner. */
    argumentsJson: string
    toolId: string
    title: string
    options: { id: string; label: string }[]
    /**
     * Present when the review answers itself if nobody does.
     * `reason` is the past-tense record. The pending line is built from
     * `dueAtMs`, `effect`, and `stopsTurn`. `dueAtMs` is on this panel's
     * clock, put there as the view arrived (`gatewayEffects`).
     */
    deadline?: {
      waitMs: number
      dueAtMs: number
      effect: "allow" | "deny"
      reason: string
      stopsTurn: boolean
    }
  }[]
  /**
   * Questions the view lists whose content has not been read yet: no answer
   * is offered to one until it has. `failed` while a read is being retried.
   */
  questionsWaiting: { question: string; message: string; failed: boolean }[]
  /**
   * Questions the agent is waiting on, each read whole from its live owner.
   * Nothing is authorised by answering.
   */
  questions: {
    /** The key the view lists it under; an answer names it. */
    question: string
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
    /** The MCP server and tool the call went to, when the harness named them. */
    mcp?: { server: string; tool: string; resourceUri?: string }
    /** The call's structured result as JSON text; its text stays in `details`. */
    structuredContent?: string
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
  interactionViewError?: string
}

export type SubmissionReceipt = {
  readonly conversationId: string
  readonly executionId: string
  readonly disposition: string
}

/**
 * One read of the list: its rows, and whether the gateway says they are every
 * conversation under that filter — not so past its bound of 500, or while one
 * of the caller's own records or summaries cannot be read back.
 */
export type ConversationListing = {
  conversations: ConversationSummary[]
  complete: boolean
}

/**
 * One conversation as the gateway lists it: a row for the Messages list. The
 * gateway reads these from what it has stored, so a closed conversation is
 * listed too, and listing starts nothing.
 */
export type ConversationSummary = {
  /** The gateway's identity for the conversation, not a tab id. */
  conversationId: string
  /** Derived by the gateway from the first message; null before anything was said. */
  title: string | null
  /** The last thing said, as one plain line; null when there is none on record. */
  preview: string | null
  updatedAtMs: number
  running: boolean
  archived: boolean
}
