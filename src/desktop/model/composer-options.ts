import catalog from "../../../crates/nessa-sdk/data/models.json"
import type { AgentId } from "../../onboarding/model/onboarding"

/**
 * What the composer offers: the SDK's model catalog, grouped by provider, and
 * the thinking levels and Fast mode each model is published with. The catalog
 * is read from the file the SDK owns, never retyped here.
 */
export interface ComposerModel {
  provider: string
  modelId: string
  displayName: string
  /** `null` for a model that does not reason; its levels, provider's order, where it does. */
  reasoning: { effortLevels: readonly string[] } | null
  /** Whether the provider publishes a fast mode for the model. */
  fastMode: boolean
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
  /** The provider's own name for the level, as the catalogue records it. */
  readonly value: string
  readonly label: string
  readonly description: string
  /**
   * Past Max: a level the model's provider publishes beyond its `max`, the
   * most it will think. The thinking control sets it apart as Ultra. Only a
   * model whose catalogue entry lists one has it.
   */
  readonly utmost?: true
}

/**
 * How the thinking control words the level names providers publish, least
 * first. This is the control's look, not the SDK's: the catalogue keeps each
 * provider's names and order (ADR 302), and nothing here adds a level to a
 * model — a model is offered exactly the levels its entry lists. The order
 * here only says how near one name stands to another, for a choice carried
 * from one model to the next (`offeredLevelIndex`).
 */
const levelWords: readonly { name: string; label: string; description: string }[] = [
  { name: "none", label: "None", description: "Answers without thinking" },
  { name: "low", label: "Low", description: "Quick answers" },
  { name: "medium", label: "Medium", description: "Balanced" },
  { name: "high", label: "High", description: "Works it through" },
  { name: "xhigh", label: "Extra high", description: "Stays with long, hard work" },
  { name: "max", label: "Max", description: "Thinks as long as it needs" },
]

export const defaultThinkingLevel = "medium"

/**
 * The levels a model accepts, least first: exactly those its catalogue entry
 * lists, in its provider's order — none for a model that does not reason or
 * has none recorded. A level listed after `max` is Ultra (`utmost`). A name
 * with no words above is shown as the provider writes it.
 */
export function thinkingLevelsFor(
  model: ComposerModel | undefined,
): readonly ThinkingLevel[] {
  const names = model?.reasoning?.effortLevels ?? []
  const max = names.indexOf("max")
  return names.map((name, index) => {
    const words = levelWords.find((entry) => entry.name === name)
    const utmost = max >= 0 && index > max
    return {
      value: name,
      label: words?.label ?? name.charAt(0).toUpperCase() + name.slice(1),
      description: words?.description ?? (utmost ? "Its deepest thinking, past Max" : ""),
      ...(utmost ? { utmost: true as const } : {}),
    }
  })
}

/**
 * Which of `levels` stands for `value`: the level itself, or — for a level
 * this model does not offer — the highest it offers below it, so a choice
 * carried across a change of model is shown as near as the model allows and
 * never as a lower one than it can give; the first level where it offers
 * none below. Only levels whose names are worded (`levelWords`) stand below
 * or above anything. An unknown value is the first level.
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
    const standing = levelRank(level.value)
    if (standing >= 0 && standing <= rank) below = index
  })
  return below
}

/**
 * Where a level's name stands among the names the control words, least
 * first — the same whichever model offers it; -1 for a name it does not word.
 */
export function levelRank(value: string | undefined): number {
  return levelWords.findIndex((entry) => entry.name === value)
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
