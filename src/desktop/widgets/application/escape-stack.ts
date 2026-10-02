/**
 * A host's Escape for the view it draws (ADR 326): a view with somewhere to
 * step back to — a detail it opened — registers that step back with
 * `onEscape` while it has it, and takes it away when it is gone. Escape that
 * reaches the host runs the step registered last, and only that; with none,
 * the host's own Escape follows (the window goes back to the panes; a pane
 * does nothing).
 */
export interface EscapeStack {
  /** Registers a step back, run before any registered earlier; returns its removal. */
  push(stepBack: () => void): () => void
  /** Runs the step back registered last; false when none is. */
  escape(): boolean
}

export function escapeStack(): EscapeStack {
  // Each registration its own entry, so one handler registered twice is two.
  const steps: { readonly run: () => void }[] = []
  return {
    push: (run) => {
      const entry = { run }
      steps.push(entry)
      return () => {
        const at = steps.indexOf(entry)
        if (at >= 0) steps.splice(at, 1)
      }
    },
    escape: () => {
      const top = steps.at(-1)
      if (!top) return false
      top.run()
      return true
    },
  }
}
