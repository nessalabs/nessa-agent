import type { ApprovalMode } from "./types"

/**
 * What the gateway will run, as `agents.list` reports it (ADR 231 §1): each
 * configured agent, the catalog models it may run, the one it runs when none is
 * named, and the approval modes it can honour. The panel renders this and holds
 * no model table of its own.
 */
export type ModelCatalog = {
  agents: CatalogAgent[]
  /**
   * The agent this window creates a conversation with when nobody chose one:
   * the agent picked at setup. It is the window's fact, not the gateway's, so
   * whoever loads the catalog joins the two.
   */
  defaultAgent: string
}

export type CatalogAgent = {
  agent: string
  models: CatalogModel[]
  defaultModel: string
  approvalModes: ApprovalMode[]
}

export type CatalogModel = {
  modelId: string
  displayName: string
  maxContextWindowTokens: number
  reasoning: boolean
  imageInput: boolean
}

/** An agent and one of its models, as someone picked them for a conversation. */
export type ModelChoice = { agent: string; model: string }

/**
 * Whether a conversation's model can still be chosen. A conversation is created
 * with its model (ADR 231 §2), so until creation starts choosing is free;
 * afterwards its model is fixed, and a different choice is a new tab.
 *
 * Creation starts when the tab is bound to a server id, which every path does
 * before it calls `conversation.create` — a send, and an attachment staged
 * before anything is sent. Waiting for the gateway's answer instead would let a
 * choice land while a create already carrying the earlier one is in flight.
 */
export function modelChoiceOpen(conversation: {
  serverConversationId?: string
  turns: readonly unknown[]
}): boolean {
  return !conversation.serverConversationId && conversation.turns.length === 0
}

/**
 * The approval mode a conversation not yet created will start in, and the ones
 * it may choose from: those its chosen agent honours, per the catalog. A mode
 * the agent does not honour, such as one kept from a different agent, falls
 * back to asking first (ADR 231 §3's default).
 *
 * Undefined once the conversation exists: changing it then is the gateway's
 * `conversation.setApprovalMode`, and until the panel can send that it offers
 * no change it cannot make.
 */
export function approvalChoice(
  conversation: {
    serverConversationId?: string
    turns: readonly unknown[]
    modelChoice?: ModelChoice
    approvalChoice?: ApprovalMode
    remote?: { runtime?: { agent?: string; model: string } }
  },
  catalog: ModelCatalog | undefined,
): { mode: ApprovalMode; modes: ApprovalMode[] } | undefined {
  if (!modelChoiceOpen(conversation)) return undefined
  const agent = effectiveModel(conversation, catalog)?.agent
  const modes = catalog?.agents.find((entry) => entry.agent === agent)?.approvalModes
  if (!modes?.length) return undefined
  const chosen = conversation.approvalChoice
  return { mode: chosen && modes.includes(chosen) ? chosen : "ask", modes }
}

/**
 * The model a conversation runs on, or will be created with: what the gateway
 * reports once it exists, else what was chosen, else the catalog's default.
 * Undefined when none of those can be named, such as before a catalog loads.
 */
export function effectiveModel(
  conversation: {
    serverConversationId?: string
    turns: readonly unknown[]
    modelChoice?: ModelChoice
    remote?: { runtime?: { agent?: string; model: string } }
  },
  catalog: ModelCatalog | undefined,
): ModelChoice | undefined {
  const runtime = conversation.remote?.runtime
  if (runtime?.agent) return { agent: runtime.agent, model: runtime.model }
  if (!modelChoiceOpen(conversation)) return undefined
  if (conversation.modelChoice) return conversation.modelChoice
  const fallback = catalog?.agents.find((entry) => entry.agent === catalog.defaultAgent)
  return fallback ? { agent: fallback.agent, model: fallback.defaultModel } : undefined
}
