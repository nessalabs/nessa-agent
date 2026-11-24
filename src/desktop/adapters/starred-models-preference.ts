import type { ModelRef } from "../workspace/model/workspace-index"
import { storedPreference } from "./stored-preference"

/** At most this many stars are kept: the picker's Starred list is a short list, not a second catalogue. */
export const starredModelsLimit = 24

/**
 * The starred models, in the order they were starred, from whatever storage
 * held. Anything that is not a list of `{ provider, modelId }` strings is
 * dropped entry by entry, so one bad entry never costs the rest; repeats keep
 * their first place, and past the limit the newest are kept.
 */
export function parseStarredModels(stored: unknown): readonly ModelRef[] {
  if (typeof stored !== "string") return []
  let entries: unknown
  try {
    entries = JSON.parse(stored)
  } catch {
    return []
  }
  if (!Array.isArray(entries)) return []
  const starred: ModelRef[] = []
  for (const entry of entries) {
    if (typeof entry !== "object" || entry === null) continue
    const { provider, modelId } = entry as Record<string, unknown>
    if (typeof provider !== "string" || typeof modelId !== "string") continue
    if (starred.some((each) => each.provider === provider && each.modelId === modelId))
      continue
    starred.push({ provider, modelId })
  }
  return starred.slice(-starredModelsLimit)
}

/**
 * The models starred in the composer's picker, remembered like the window's
 * other preferences and shared by every pane: a star made in one pane shows
 * in all of them. A star on a model the catalogue no longer lists is kept,
 * and shown again if the model comes back.
 */
const starredModelsPreference = storedPreference<readonly ModelRef[]>({
  key: "nessa.desktop.starred-models",
  event: "nessa:desktop-starred-models",
  parse: parseStarredModels,
  serialize: (starred) =>
    JSON.stringify(
      // The newest stars, so a new one is never dropped for an old one.
      starred
        .slice(-starredModelsLimit)
        .map(({ provider, modelId }) => ({ provider, modelId })),
    ),
})

export const useStarredModels = starredModelsPreference.usePreference
