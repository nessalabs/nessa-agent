import { EmptyState } from "@nessa-ui/react/empty-state"
import { DesktopIcon } from "../../../ui/icons"

/**
 * What "Needs you" says when nothing does: a lit check and one quiet line,
 * the reward for a list cleared — never an empty box.
 */
export function AllClear() {
  return (
    <EmptyState
      className="agents-clear"
      data-reflow="all-clear"
      role="status"
      title={
        <>
          <span className="agents-clear-mark" aria-hidden="true">
            <DesktopIcon name="check" />
          </span>
          Nothing needs you
        </>
      }
      description="When an agent asks to run something, or has a question, it waits here."
    />
  )
}
