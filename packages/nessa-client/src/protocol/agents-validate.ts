import { InstallableAgent, bounds } from "../generated/product.js"
import type {
  AgentInstallOptionsResult,
  AgentInstallResult,
  AgentsListResult,
  ApprovalModeChoice,
} from "../generated/product.js"

const utf8 = new TextEncoder()
const modes = new Set(["ask", "auto", "full"])

function object(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new Error("Invalid agent catalog response")
  return value as Record<string, unknown>
}

function exact(value: Record<string, unknown>, keys: readonly string[]) {
  if (Object.keys(value).some((key) => !keys.includes(key)))
    throw new Error("Agent catalog response has unknown fields")
}

function text(value: Record<string, unknown>, key: string, max: number): string {
  const field = value[key]
  if (typeof field !== "string" || !field || utf8.encode(field).byteLength > max)
    throw new Error(`Invalid agent catalog ${key}`)
  return field
}

/** Validate the same model-specific choices on a catalog row or conversation view. */
export function approvalModeChoices(value: unknown): ApprovalModeChoice[] {
  if (!Array.isArray(value) || !value.length || value.length > 3)
    throw new Error("Invalid agent approval modes")
  const seen = new Set<string>()
  for (const entry of value) {
    const item = object(entry)
    exact(item, ["id", "name", "description"])
    const id = text(item, "id", 4)
    if (!modes.has(id) || seen.has(id)) throw new Error("Invalid agent approval mode")
    seen.add(id)
    text(item, "name", 64)
    text(item, "description", 512)
  }
  if (!seen.has("ask")) throw new Error("Agent has no default approval mode")
  return value as ApprovalModeChoice[]
}

/** Validate one authenticated gateway catalog before it reaches a picker. */
export function agentsList(value: unknown): AgentsListResult {
  const result = object(value)
  exact(result, ["agents"])
  if (!Array.isArray(result.agents) || result.agents.length > bounds.maxConfiguredAgents)
    throw new Error("Invalid configured agents")
  const agents = new Set<string>()
  for (const entry of result.agents) {
    const agent = object(entry)
    exact(agent, ["agent", "defaultModel", "models"])
    const name = text(agent, "agent", 32)
    if (agents.has(name)) throw new Error("Repeated configured agent")
    agents.add(name)
    const defaultModel = text(agent, "defaultModel", 256)
    if (!Array.isArray(agent.models) || !agent.models.length || agent.models.length > 256)
      throw new Error("Invalid agent models")
    const models = new Set<string>()
    for (const entry of agent.models) {
      const model = object(entry)
      exact(model, [
        "modelId",
        "displayName",
        "maxContextWindowTokens",
        "reasoning",
        "imageInput",
        "approvalModes",
      ])
      const id = text(model, "modelId", 256)
      if (models.has(id)) throw new Error("Repeated agent model")
      models.add(id)
      text(model, "displayName", 256)
      if (
        !Number.isSafeInteger(model.maxContextWindowTokens) ||
        (model.maxContextWindowTokens as number) < 1 ||
        (model.maxContextWindowTokens as number) > 4294967295 ||
        typeof model.reasoning !== "boolean" ||
        typeof model.imageInput !== "boolean"
      )
        throw new Error("Invalid agent model capabilities")
      approvalModeChoices(model.approvalModes)
    }
    if (!models.has(defaultModel)) throw new Error("Agent default model is unavailable")
  }
  return result as unknown as AgentsListResult
}

const installAgents = new Set<string>(Object.values(InstallableAgent))

/** Validate supported offers before presenting download sizes or actions. */
export function agentInstallOptions(value: unknown): AgentInstallOptionsResult {
  const result = object(value)
  exact(result, ["agents"])
  if (!Array.isArray(result.agents)) throw new Error("Invalid installation offers")
  const seen = new Set<string>()
  for (const entry of result.agents) {
    const offer = object(entry)
    exact(offer, ["agent", "version", "archiveBytes", "installed"])
    const agent = text(offer, "agent", 32)
    if (!installAgents.has(agent) || seen.has(agent))
      throw new Error("Invalid installation agent")
    seen.add(agent)
    text(offer, "version", bounds.maxAgentInstallVersionBytes)
    if (
      typeof offer.installed !== "boolean" ||
      typeof offer.archiveBytes !== "number" ||
      !Number.isSafeInteger(offer.archiveBytes) ||
      offer.archiveBytes < 1
    )
      throw new Error("Invalid installation offer")
  }
  return value as AgentInstallOptionsResult
}

/** Validate confirmed installation independently of agent authentication. */
export function agentInstallResult(value: unknown): AgentInstallResult {
  const result = object(value)
  exact(result, ["agent", "version", "downloaded", "cleanupPending"])
  if (!installAgents.has(text(result, "agent", 32)))
    throw new Error("Invalid installation agent")
  text(result, "version", bounds.maxAgentInstallVersionBytes)
  if (
    typeof result.downloaded !== "boolean" ||
    typeof result.cleanupPending !== "boolean"
  )
    throw new Error("Invalid installation result")
  return value as AgentInstallResult
}
