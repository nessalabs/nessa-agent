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
 * with its model on the first send (ADR 231 §2), so until then choosing is
 * free; afterwards its model is fixed, and a different choice is a new tab.
 */
export function modelChoiceOpen(conversation: {
  serverReady?: boolean
  turns: readonly unknown[]
}): boolean {
  return !conversation.serverReady && conversation.turns.length === 0
}

/**
 * The model a conversation runs on, or will be created with: what the gateway
 * reports once it exists, else what was chosen, else the catalog's default.
 * Undefined when none of those can be named, such as before a catalog loads.
 */
export function effectiveModel(
  conversation: {
    serverReady?: boolean
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
