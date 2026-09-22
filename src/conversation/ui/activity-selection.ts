import type { AgentTurnContentView } from "./agent-transcript-view"

type ActivityPart = Extract<AgentTurnContentView, { activity: unknown }>

/** Resolve one turn's selected activity from the latest replacement projection. */
export function selectedTurnActivity(
  segments: readonly AgentTurnContentView[],
  key: string | null,
): ActivityPart | undefined {
  if (key === null) return undefined
  const selected = segments.find((part) => part.key === key)
  return selected && "activity" in selected ? selected : undefined
}
