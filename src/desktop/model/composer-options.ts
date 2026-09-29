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

/** A thinking level a reasoning model accepts, as the composer offers it. */
export interface ThinkingLevel {
  readonly value: string
  readonly label: string
  readonly description: string
  /**
   * Beyond Max: the most any model will think, offered only by a model that
   * has it (`ultraThinkingFor`). The thinking control sets it apart.
   */
  readonly utmost?: true
}

export const thinkingLevels: readonly ThinkingLevel[] = [
  { value: "low", label: "Low", description: "Quick answers" },
  { value: "medium", label: "Medium", description: "Balanced" },
  { value: "high", label: "High", description: "Works it through" },
  { value: "max", label: "Max", description: "Thinks as long as it needs" },
  {
    value: "ultra",
    label: "Ultra",
    description: "Its deepest thinking, past Max",
    utmost: true,
  },
]

export const defaultThinkingLevel = "medium"

/**
 * Models that offer Ultra thinking, past Max. The SDK catalog records only
 * whether a model reasons — by design it prescribes no provider's effort
 * levels (`crates/nessa-sdk/README.md`) — so, as with Fast mode below, they
 * are listed here by provider and model until the catalog carries it.
 */
export const ultraThinkingModels: readonly { provider: string; modelId: string }[] = [
  { provider: "anthropic", modelId: "claude-opus-5" },
  { provider: "openai", modelId: "gpt-6-astra" },
]

/** Whether a model thinks past Max, at Ultra. */
export function ultraThinkingFor(model: ComposerModel | undefined): boolean {
  return listed(ultraThinkingModels, model)
}

/**
 * The levels a model accepts: none for a model that does not reason, and
 * Ultra only for one that has it.
 */
export function thinkingLevelsFor(
  model: ComposerModel | undefined,
): readonly ThinkingLevel[] {
  if (!model?.reasoning) return []
  const ultra = ultraThinkingFor(model)
  return thinkingLevels.filter((level) => !level.utmost || ultra)
}

/**
 * Which of `levels` stands for `value`: the level itself, or — for a level
 * this model does not offer, Ultra on a model that stops at Max — the highest
 * it offers below it, so a choice carried across a change of model is shown
 * as near as the model allows and never as a lower one than it can give.
 * An unknown value is the first level.
 */
export function offeredLevelIndex(
  levels: readonly ThinkingLevel[],
  value: string,
): number {
  const exact = levels.findIndex((level) => level.value === value)
  if (exact >= 0) return exact
  const rank = levelRank(value)
  if (rank < 0) return 0
  let below = 0
  levels.forEach((level, index) => {
    if (levelRank(level.value) <= rank) below = index
  })
  return below
}

/**
 * Where a level stands among all the levels there are, least first — the
 * same whichever model offers it; -1 for a value that is not a level.
 */
export function levelRank(value: string | undefined): number {
  return thinkingLevels.findIndex((level) => level.value === value)
}

const listed = (
  entries: readonly { provider: string; modelId: string }[],
  model: ComposerModel | undefined,
) =>
  model !== undefined &&
  entries.some(
    (entry) => entry.provider === model.provider && entry.modelId === model.modelId,
  )

/**
 * Models that offer Fast mode. The SDK catalog does not record it yet, so it
 * is listed here by provider and model until the catalog carries it.
 */
export const fastModeModels: readonly { provider: string; modelId: string }[] = [
  { provider: "anthropic", modelId: "claude-opus-5" },
]

/** Whether the thinking control offers Fast mode for this model. */
export function fastModeFor(model: ComposerModel | undefined): boolean {
  return listed(fastModeModels, model)
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
