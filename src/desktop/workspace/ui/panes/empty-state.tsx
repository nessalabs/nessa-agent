import { loadWorkspace } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import type { WorkspaceFailureReason } from "../../model/failure"
import { failureCopy } from "../failure-copy"

/**
 * What the workspace shows when there is no conversation to show: its
 * sessions could not be read. It says so plainly and offers the one thing to
 * do next. (No pane shows a session that is not there: a removal starts the
 * last pane over — `usecases/updates.ts`.)
 */
export function EmptyWorkspace({ failure }: { failure: WorkspaceFailureReason | null }) {
  const dispatch = useWorkspaceDispatch()
  if (!failure) return null
  return (
    <div className="workspace-empty" role="status">
      <p>{failureCopy(failure)}</p>
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
