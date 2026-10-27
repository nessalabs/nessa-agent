import { ChevronLeft, X } from "lucide-react"

/**
 * Setup's own chrome: the way back and the way out — round glass buttons
 * floating in the window's top corners, Back on the left and Close on the
 * right, where a sheet on a phone keeps them.
 *
 * It belongs to the window rather than to the step, so a control that is
 * always there is not re-mounted by every step. It comes in with the pane once
 * the window has formed (`styles.css`): over the glow, buttons hanging in
 * empty space gave the shape away before the light did. Escape leaves from the
 * first frame. While the finished window flies into the menu bar neither is
 * drawn, as there is nothing left to go back to or close.
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
