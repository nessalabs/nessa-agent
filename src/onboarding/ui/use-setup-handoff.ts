import * as React from "react"
import {
  closeSetupWindow,
  finishSetupWindow,
  revealSetupWindow,
  type SetupHandoff,
} from "../../host"
import {
  applySetupHandoff,
  createHandoffGate,
  type HandoffGate,
  handoffRejection,
  noSetupHandoff,
  shouldHandOver,
} from "../application/setup-handoff"
import { setupRecovery, type SetupRecovery } from "../application/setup-recovery"

/** What the setup surface needs in order to draw itself and offer its ways out. */
export interface SetupHandoffCoordination {
  /** The screen the handoff left, or `null` when it left none. */
  recovery: SetupRecovery | null
  /** True once a close was asked for and the window system did not do it. */
  closeFailed: boolean
  /** The alert, so the caller can hand it the focus setup's controls took away. */
  dialog: React.RefObject<HTMLDivElement | null>
  /** Ask for the handoff again, from the beginning. */
  retry: () => void
  /** Close this window without the panel. */
  close: () => void
}

/**
 * Everything the end of setup does to the window it is painted in.
 *
 * The window is revealed once this has rendered, the handoff is asked for when
 * setup is over, and whatever screen that leaves is given the caret. Which of
 * those to do is decided in `application/setup-handoff.ts`; this only performs
 * them, so the component that draws the screen takes props and nothing else.
 *
 * What is decided is tested there, including the one-call-per-attempt gate. What
 * is performed here is not: this suite has no DOM, so the reveal and the focus
 * are covered by reading, as they were before this was extracted. Moving them
 * did not make those testable.
 *
 * @param setupActive Whether setup is still on screen.
 * @param completed Whether setup was finished rather than left. The one fact
 *   the host cannot know, and the first of the two things the handoff carries.
 * @param agent The agent setup finished on, when it finished on one. The
 *   second, and absent whenever `completed` is false: a choice made on the way
 *   out of setup is not a decision, so there is nothing for the host to record.
 */
export function useSetupHandoff(
  setupActive: boolean,
  completed: boolean,
  agent?: string,
): SetupHandoffCoordination {
  const [state, record] = React.useReducer(applySetupHandoff, noSetupHandoff)
  const dialog = React.useRef<HTMLDivElement>(null)
  // One host call per attempt, however many times the effect below runs against
  // the same state. See `createHandoffGate`. Built on the first render only, so
  // re-rendering cannot hand the effect a gate that is holding nothing.
  const held = React.useRef<HandoffGate>(undefined)
  held.current ??= createHandoffGate()
  const gate = held.current

  // The window is created hidden and shown from here, after this has rendered
  // — so the first thing on screen is the opening rather than an empty window
  // waiting for its first frame.
  //
  // Directly, in the effect, and deliberately not from `requestAnimationFrame`.
  // A hidden macOS window is not drawn at all, so its webview is served no
  // animation frames: a reveal scheduled on one waits for a paint that is
  // waiting for the reveal, and setup stayed hidden for the whole session while
  // its page ran and played the opening sound.
  //
  // The body is a block so the effect returns nothing: an expression body would
  // hand React the promise as a cleanup function.
  React.useEffect(() => {
    void revealSetupWindow()
  }, [])

  // Handing over to the panel. Showing it, writing setup off for good, and
  // closing this window are one host call, because the order between them has
  // to survive this window — see `finishSetupWindow` and `panel::finish_setup`.
  // This sequenced them itself until a completion write issued after an awaited
  // close started losing to the teardown it had just asked for.
  //
  // What travels is what the host cannot know: whether setup was finished or
  // left, and the agent it was finished on. Leaving stays free to change its
  // mind, so only a finish is written off; the host will not record it unless
  // the panel came up first.
  React.useEffect(() => {
    if (!shouldHandOver(state, setupActive) || !gate.claim()) return
    // The attempt this answer belongs to. `finishSetupWindow` cannot be
    // cancelled, so a call outlives the attempt that made it: the number is how
    // an answer to an abandoned attempt is recognised and dropped instead of
    // being read as the answer to the current one.
    const attempt = state.attempts + 1
    record({ type: "requested" })
    const settle = (outcome: SetupHandoff) => {
      gate.release()
      record({ type: "settled", attempt, outcome })
    }
    void finishSetupWindow(completed, agent)
      .then(settle)
      .catch((cause: unknown) => settle(handoffRejection(cause)))
  }, [state, setupActive, completed, agent, gate])

  // What the window has to show for the handoff it got, if anything. Kept by
  // the handoff rather than recomputed every render, so a failed close — which
  // changes the note under the buttons — does not take the focus back off the
  // button somebody just pressed.
  const recovery = React.useMemo(() => setupRecovery(state.outcome), [state.outcome])

  // An alert dialog nobody is inside is one a screen reader reads past, and
  // setup's own controls have gone with the step that had them: there is
  // nothing for the caret to fall back to but this.
  React.useEffect(() => {
    if (recovery) dialog.current?.focus()
  }, [recovery])

  // The gate is deliberately not released here; see `createHandoffGate`. A retry
  // is only offered once an answer is on screen, which is after the call it
  // answered released the gate, so there is nothing to release anyway.
  const retry = React.useCallback(() => record({ type: "retry" }), [])

  // Closing without the panel. Every way this can fail is a value rather than a
  // rejection, so the screen can say what happened and keep its buttons.
  const close = React.useCallback(() => {
    void closeSetupWindow().then((close) => record({ type: "closed", close }))
  }, [])

  return { recovery, closeFailed: state.closeFailed, dialog, retry, close }
}
