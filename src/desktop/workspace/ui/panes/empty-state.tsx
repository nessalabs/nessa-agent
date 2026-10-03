import { loadWorkspace } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import type { WorkspaceFailureReason } from "../../model/failure"
import { failureCopy } from "../failure-copy"

/**
 * What the workspace shows when there is no conversation to show: its
 * sessions could not be read. It says so plainly — signed out, or no answer
 * from the local server, never the sample in its place (#419) — and offers
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

/**
 * `failureCopy`'s sentence, except where a read had no answer: there the
 * window says what it could not read, and from where, rather than that a
 * call is unconfirmed.
 */
function readFailureCopy(failure: WorkspaceFailureReason): string {
  return failure === "unavailable"
    ? "Nessa couldn’t read the local server’s conversations just now."
    : failureCopy(failure)
}
