import { DesktopIcon } from "../../../ui/icons"

/**
 * What "Needs you" says when nothing does: a lit check and one quiet line,
 * the reward for a list cleared — never an empty box.
 */
export function AllClear() {
  return (
    <div className="agents-clear" data-reflow="all-clear" role="status">
      <span className="agents-clear-mark" aria-hidden="true">
        <DesktopIcon name="check" />
      </span>
      <p className="agents-clear-title">Nothing needs you</p>
      <p className="agents-clear-detail">
        When an agent asks to run something, or has a question, it waits here.
      </p>
    </div>
  )
}
