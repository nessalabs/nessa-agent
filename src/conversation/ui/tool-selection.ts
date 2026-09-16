/** Resolve a selected tool group from the latest replacement projection. */
export function selectedToolActivity<T extends { key: string; tools?: unknown[] }>(
  segments: readonly T[],
  key: string | null,
): T | undefined {
  return key === null
    ? undefined
    : segments.find((part) => part.key === key && part.tools)
}
