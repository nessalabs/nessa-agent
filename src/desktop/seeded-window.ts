/**
 * Boots the desktop window on a seeded in-memory workspace (#595).
 *
 * The page query is `seededWorkspaceSpec`'s. This module publishes the
 * builder's report and the module URL, then hands `inMemorySource` the
 * built index. It does not open a gateway and it does not call
 * `conversation.list`.
 */
import {
  inMemorySource,
  seededWorkspace,
  seededWorkspaceSpec,
  SeededWorkspaceRefusal,
  type WorkspaceSource,
} from "./workspace"

export interface SeededWindow {
  readonly workspace: WorkspaceSource
  readonly now: () => number
}

function schedule(now: () => number) {
  return {
    now,
    after: (ms: number, run: () => void) => {
      const timer = window.setTimeout(run, ms)
      return () => window.clearTimeout(timer)
    },
  }
}

/** The workspace and clock for a seeded page. A refusal is written on the document and rethrown. */
export function seededWindow(search: string): SeededWindow {
  try {
    const spec = seededWorkspaceSpec(search)
    if (!spec) throw new SeededWorkspaceRefusal("seed")
    const built = seededWorkspace(spec)
    document.documentElement.dataset.uiRevision = import.meta.url
    document.documentElement.dataset.seededReport = JSON.stringify(built.report)
    const now = () => spec.now
    return {
      workspace: inMemorySource(schedule(now), {
        index: built.index,
        transcripts: new Map(built.transcripts),
      }),
      now,
    }
  } catch (error) {
    if (error instanceof SeededWorkspaceRefusal)
      document.documentElement.dataset.seededRefusal = error.reason
    throw error
  }
}
