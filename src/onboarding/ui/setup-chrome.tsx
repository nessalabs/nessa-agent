import { ChevronLeft, X } from "lucide-react"

/**
 * Setup's own chrome: the way back and the way out — round glass buttons
 * floating in the window's top corners, Back on the left and Close on the
 * right, where a sheet on a phone keeps them.
 *
 * It belongs to the window rather than to the step, for two reasons. A control
 * that is always there should not be re-mounted by every step, and — the one
 * that matters — the wash the steps are painted on fades in behind the whole
 * opening, which took the only visible exit with it. Sitting on the window
 * instead, these are on screen as soon as the window is, which is what a
 * full-screen surface covering the menu bar owes the person looking at it.
 *
 * Drawn window controls were never convincing — they have to reimplement every
 * state the system already draws, and the one that could not do anything had to
 * either look broken or lie. Two quiet marks ask for none of that.
 */
export function SetupChrome({
  onBack,
  onClose,
}: {
  /** Back one step. Absent on a step with nothing before it, and then no
   * button is drawn rather than one that does nothing. */
  onBack?: () => void
  onClose?: () => void
}) {
  return (
    <div className="nessa-setup-chrome">
      {/* Each corner keeps its place whether or not it holds a button, so the
        close never slides over when Back comes and goes. */}
      <div className="nessa-setup-corner">
        {onBack ? (
          <button
            type="button"
            aria-label="Back"
            onClick={onBack}
            className="nessa-setup-control nessa-setup-back"
          >
            <ChevronLeft aria-hidden className="nessa-setup-glyph" />
          </button>
        ) : null}
      </div>
      <div className="nessa-setup-corner">
        {onClose ? (
          <button
            type="button"
            aria-label="Close setup"
            onClick={onClose}
            className="nessa-setup-control"
          >
            <X aria-hidden className="nessa-setup-glyph" />
          </button>
        ) : null}
      </div>
    </div>
  )
}
