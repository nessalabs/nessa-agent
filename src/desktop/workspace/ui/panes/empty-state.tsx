import { loadWorkspace } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import type { WorkspaceFailureReason } from "../../model/failure"
import { readFailureCopy } from "../failure-copy"

/**
 * What the workspace shows when there is no conversation to show: its
 * sessions could not be read. It says so plainly — signed out, or no answer
 * from the local server; in the desktop app this is drawn instead of the
 * sample, which `verification/desktop/scripts/gateway-states.mjs` checks
 * (#419) — and offers
 * the one thing to do next. (No pane shows a session that is not there: a
 * removal starts the last pane over — `usecases/updates.ts`.)
 */
export function EmptyWorkspace({ failure }: { failure: WorkspaceFailureReason | null }) {
  const dispatch = useWorkspaceDispatch()
  if (!failure) return null
  return (
    <div className="workspace-empty" role="status">
      <p>{readFailureCopy(failure)}</p>
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
