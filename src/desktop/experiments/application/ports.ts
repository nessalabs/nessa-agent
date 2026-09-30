import type { Experiment } from "../model/experiment"

/**
 * Where experiments come from. The in-memory adapter plays a scripted swarm
 * today; a gateway adapter reading real eval runs takes its place later.
 * `get` answers from what the source already holds, so a view can read it on
 * every render; `subscribe` says when that changed.
 */
export interface ExperimentSource {
  get(id: string): Experiment | undefined
  /** The experiment a conversation runs, if it runs one. */
  bySession(sessionId: string): Experiment | undefined
  subscribe(listener: () => void): () => void
  /**
   * Opens a run's change in the person's editor: one file's diff, or the
   * whole change when no path is named. Diffs are read there, not drawn in
   * the experiment, until the window has an editor of its own.
   */
  openInEditor(target: EditorTarget): void
}

export interface EditorTarget {
  readonly experimentId: string
  readonly runId: string
  readonly path?: string
}
