import type { ModelThinkingLevel } from "@nessa-ui/react/model-capabilities"
import catalog from "../../../crates/nessa-sdk/data/models.json"
import type { AgentId } from "../../onboarding/model/onboarding"

/**
 * What the composer offers: the SDK's model catalog, grouped by provider, and
 * the thinking levels a reasoning model accepts. The catalog is read from the
 * file the SDK owns, never retyped here.
 */
export interface ComposerModel {
  provider: string
  modelId: string
  displayName: string
  reasoning: boolean
  maxContextWindowTokens: number
}

export interface ComposerProvider {
  id: string
  label: string
  models: ComposerModel[]
}

const providerLabels: Record<string, string> = {
  anthropic: "Anthropic",
  openai: "OpenAI",
  opencode: "OpenCode",
}

/** Names a provider for people; an unlisted one is shown by its id. */
export function providerLabel(id: string): string {
  return Object.hasOwn(providerLabels, id) ? providerLabels[id] : id
}

const providerAgents: Record<string, AgentId> = {
  anthropic: "claude",
  openai: "codex",
  opencode: "opencode",
}

/** The agent whose mark stands for a catalog provider, if nessa runs one for it. */
export function agentForProvider(id: string): AgentId | undefined {
  return Object.hasOwn(providerAgents, id) ? providerAgents[id] : undefined
}

/** Groups models by provider, keeping the catalog's order within and between groups. */
export function groupByProvider(models: readonly ComposerModel[]): ComposerProvider[] {
  const groups: ComposerProvider[] = []
  for (const model of models) {
    const group = groups.find((candidate) => candidate.id === model.provider)
    if (group) group.models.push(model)
    else
      groups.push({
        id: model.provider,
        label: providerLabel(model.provider),
        models: [model],
      })
  }
  return groups
}

export const composerModels: readonly ComposerModel[] = catalog.models
export const composerProviders = groupByProvider(composerModels)

/** Opens on Claude Opus 5 when the catalog has it, otherwise its first model. */
export function defaultComposerModel(
  models: readonly ComposerModel[],
): ComposerModel | undefined {
  return (
    models.find(
      (model) => model.provider === "anthropic" && model.modelId === "claude-opus-5",
    ) ?? models[0]
  )
}

/** "1M context", "200K context": the window rounded to what a person would say. */
export function contextLabel(tokens: number): string {
  if (tokens >= 1_000_000) return `${Math.round(tokens / 1_000_000)}M context`
  return `${Math.round(tokens / 1_000)}K context`
}

export const thinkingLevels: ModelThinkingLevel[] = [
  { value: "low", label: "Low", description: "Quick answers" },
  { value: "medium", label: "Medium", description: "Balanced" },
  { value: "high", label: "High", description: "Works it through" },
  {
    value: "max",
    label: "Max",
    description: "Thinks as long as it needs",
    accent: "ultra",
  },
]

export const defaultThinkingLevel = "medium"

/** A model that does not reason offers no levels, and the control says so. */
export function thinkingLevelsFor(
  model: ComposerModel | undefined,
): ModelThinkingLevel[] {
  return model?.reasoning ? thinkingLevels : []
}

/**
 * Models that offer Fast mode. The SDK catalog does not record it yet, so it
 * is listed here by provider and model until the catalog carries it.
 */
const fastModeModels: readonly { provider: string; modelId: string }[] = [
  { provider: "anthropic", modelId: "claude-opus-5" },
]

/** Whether the thinking control offers Fast mode for this model. */
export function fastModeFor(model: ComposerModel | undefined): boolean {
  return (
    model !== undefined &&
    fastModeModels.some(
      (entry) => entry.provider === model.provider && entry.modelId === model.modelId,
    )
  )
}

/**
 * A model's name in few words, for a composer too narrow for the whole of it:
 * the name without the word every model of its provider starts with ("Claude
 * Opus 5" is "Opus 5"), so two models still read apart. A provider whose
 * names share no first word keeps them whole. Derived from the catalogue,
 * never written out.
 */
export function shortModelName(
  model: ComposerModel,
  models: readonly ComposerModel[] = composerModels,
): string {
  const [first, ...rest] = model.displayName.split(" ")
  const siblings = models.filter((other) => other.provider === model.provider)
  const shared =
    rest.length > 0 &&
    siblings.length > 1 &&
    siblings.every((other) => other.displayName.split(" ")[0] === first)
  return shared ? rest.join(" ") : model.displayName
}
