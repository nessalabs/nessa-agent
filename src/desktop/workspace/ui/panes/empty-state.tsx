import { loadWorkspace, newSession } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import type { PaneKey } from "../../model/pane-layout"

/**
 * What a pane or the workspace shows when there is no conversation to show:
 * a session that left the lists while its pane stayed, or a workspace whose
 * sessions could not be read. Each says so plainly and offers the one thing
 * to do next.
 */
export function EmptyPane({ pane }: { pane: PaneKey }) {
  const dispatch = useWorkspaceDispatch()
  return (
    <div className="workspace-empty" role="status">
      <p>This session is no longer here.</p>
      <button
        type="button"
        className="workspace-button"
        onClick={() => dispatch(newSession({ target: pane }))}
      >
        New Session
      </button>
    </div>
  )
}

export function EmptyWorkspace({ failure }: { failure: string | null }) {
  const dispatch = useWorkspaceDispatch()
  if (!failure) return null
  return (
    <div className="workspace-empty" role="status">
      <p>{failure}</p>
      <button
        type="button"
        className="workspace-button"
        onClick={() => void dispatch(loadWorkspace())}
      >
        Try Again
      </button>
    </div>
  )
}
