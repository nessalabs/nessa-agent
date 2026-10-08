import { EmptyState } from "@nessa-ui/react/empty-state"
import { loadWorkspace } from "../../adapters/store/commands"
import { useWorkspaceDispatch } from "../../adapters/store/hooks"
import type { StageMismatch, WorkspaceFailureReason } from "../../model/failure"
import { readFailureCopy } from "../failure-copy"
import { startupFailureCode } from "../startup-failure"

/**
 * What the workspace shows when there is no conversation to show: its
 * sessions could not be read. It says so plainly — signed out, or no answer
 * from the local server; in the desktop app this is drawn instead of the
 * sample, which `verification/desktop/scripts/gateway-states.mjs` checks
 * (#419) — and offers
 * the one thing to do next. A startup failure is the window's calm screen
 * (`ui/startup-fallback.tsx`), not this pane. (No pane shows a session that
 * is not there: a removal starts the last pane over — `usecases/updates.ts`.)
 */
export function EmptyWorkspace({
  failure,
  stages,
}: {
  failure: WorkspaceFailureReason | null
  stages?: StageMismatch | null
}) {
  const dispatch = useWorkspaceDispatch()
  if (!failure || startupFailureCode(failure)) return null
  return (
    <EmptyState
      variant="compact"
      className="workspace-empty"
      role="status"
      title={readFailureCopy(failure, "index", stages)}
      action={
        <button
          type="button"
          className="workspace-button"
          onClick={() => void dispatch(loadWorkspace())}
        >
          Try Again
        </button>
      }
    />
  )
}
