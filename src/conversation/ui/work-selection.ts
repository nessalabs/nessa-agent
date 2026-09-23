/** Resolve selected turn working from the latest replacement projection. */
export function selectedWork<T extends { key: string; work?: unknown[] }>(
  segments: readonly T[],
  key: string | null,
): T | undefined {
  return key === null ? undefined : segments.find((part) => part.key === key && part.work)
}
