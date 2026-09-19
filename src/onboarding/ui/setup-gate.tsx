import * as React from "react"
import {
  closeSetupWindow,
  finishSetupWindow,
  revealSetupWindow,
  type SetupHandoff,
} from "../../host"
import { AgentBloom } from "./agent-bloom"
import { Onboarding, SETUP_HEADING_ID } from "./onboarding"
import { revealOnFirstRender } from "./reveal-on-first-render"
import { SetupChrome } from "./setup-chrome"
import { useIntroSound } from "./use-intro-sound"
import { useOnboarding } from "./use-onboarding"
import type { AgentReadinessSource } from "../application/ports"
import { setupRecovery, type SetupRecovery } from "../application/setup-recovery"
import { isOnboardingCompleted } from "../model/onboarding"

/** The buttons on the handoff-failure screen, which are the whole point of it:
 * a window with nothing in it but a sentence is a window someone has to kill. */
const RECOVERY_BUTTON =
  "h-10 rounded-full border border-border bg-card px-5 nessa-text-3 font-medium text-foreground transition-colors outline-none hover:bg-accent hover:text-accent-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40"

/**
 * What is on screen when setup is over and this window is still here.
 *
 * What it says and which ways out it offers come from `setupRecovery`, because
 * the two endings that reach here are different news: a panel that never came
 * up can be tried again, and a panel that is already up cannot — there the one
 * thing left is this window, and offering a retry would summon what is standing
 * behind the sentence.
 *
 * The close is always here. This window is undecorated: it has no title bar and
 * no close control of its own, so a screen without that button is a window
 * somebody has to kill.
 *
 * A close that itself fails changes nothing but the note. The screen stays,
 * with its buttons still on it, rather than throwing out of an event handler
 * and taking the last controls with it.
 */
export function HandoffFailed({
  recovery,
  onRetry,
  onClose,
  closeFailed,
  ref,
}: {
  /** What happened, and whether the panel is up. */
  recovery: SetupRecovery
  onRetry: () => void
  onClose: () => void
  /** True once a close was asked for and the window system did not do it. */
  closeFailed: boolean
  /**
   * The alert itself, so whoever puts it on screen can hand it the focus the
   * setup controls just took with them. Held out here rather than focused from
   * inside, so this stays a function of its props — the way its behaviour is
   * tested at all, with no DOM in this suite.
   */
  ref?: React.Ref<HTMLDivElement>
}) {
  return (
    <div className="nessa-setup-sheet">
      <div
        ref={ref}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={SETUP_HEADING_ID}
        // Focusable only as the container of this alert, not as a stop on the
        // way through it.
        tabIndex={-1}
        className="nessa-setup-window nessa-setup-light flex flex-col items-center justify-center gap-4 overflow-y-auto bg-background p-8 text-center text-foreground outline-none"
      >
        <h1 id={SETUP_HEADING_ID} className="nessa-text-6 font-semibold">
          {recovery.heading}
        </h1>
        <p className="nessa-text-3 text-muted-foreground">{recovery.detail}</p>
        <div className="flex flex-wrap items-center justify-center gap-3">
          {recovery.panelShown ? null : (
            <button type="button" className={RECOVERY_BUTTON} onClick={onRetry}>
              Try again
            </button>
          )}
          <button type="button" className={RECOVERY_BUTTON} onClick={onClose}>
            Close this window
          </button>
        </div>
        {closeFailed ? (
          <p
            role="status"
            aria-live="polite"
            className="nessa-text-2 text-muted-foreground"
          >
            This window still would not close. If it stays in the way, quit Nessa from the
            menu bar icon and start it again.
          </p>
        ) : null}
      </div>
    </div>
  )
}

/**
 * The setup surface: first run in its own window.
 *
 * The desktop host opens this in a small centred window of its own, so setup
 * never borrows the panel's frame, tabs or composer, and the panel is not
 * mounted behind it. Finishing or skipping shows the panel and closes this
 * window. In a plain browser there is no second window, so the same component
 * hands over to `children` in place.
 *
 * Which is why `children` is optional, and why the setup window passes none.
 * Rendering the panel tree there mounted a second full application — its own
 * store, its own authenticated session — inside a window whose whole remaining
 * job was to close, and left it mounted for good if the handoff ever failed.
 * The setup window shows setup, then nothing.
 */
export function SetupGate({
  agents,
  children,
}: {
  /** Where setup asks what each agent's runtime can do. Injected, so a test
   * substitutes an answer instead of a network. */
  agents: AgentReadinessSource
  /** The panel, for a surface that has to become it in place. Omitted by the
   * desktop setup window, which closes instead. */
  children?: React.ReactNode
}) {
  const onboarding = useOnboarding(agents)
  const [handedOver, setHandedOver] = React.useState(false)
  const [handoff, setHandoff] = React.useState<SetupHandoff>()
  const [closeFailed, setCloseFailed] = React.useState(false)
  const recoveryDialog = React.useRef<HTMLDivElement>(null)

  useIntroSound(onboarding.active)

  // The window is created hidden and shown from here, after this has rendered
  // — so the first thing on screen is the opening rather than an empty window
  // waiting for its first frame.
  //
  // Directly, in the effect, and deliberately not from `requestAnimationFrame`.
  // A hidden macOS window is not drawn at all, so its webview is served no
  // animation frames: a reveal scheduled on one waits for a paint that is
  // waiting for the reveal, and setup stayed hidden for the whole session while
  // its page ran and played the opening sound. `revealOnFirstRender` is where
  // that rule is written down and tested.
  React.useEffect(() => revealOnFirstRender(() => void revealSetupWindow()), [])

  // Handing over to the panel. Showing it, writing setup off for good, and
  // closing this window are one host call, because the order between them has
  // to survive this window — see `finishSetupWindow` and `panel::finish_setup`.
  // This sequenced them itself until a completion write issued after an awaited
  // close started losing to the teardown it had just asked for.
  //
  // What travels is the one fact the host cannot know: whether setup was
  // finished or left. Leaving stays free to change its mind, so only a finish
  // is written off; the host will not record it unless the panel came up first.
  const completed = isOnboardingCompleted(onboarding.state)
  React.useEffect(() => {
    if (onboarding.active || handedOver) return
    setHandedOver(true)
    void finishSetupWindow(completed)
      .then(setHandoff)
      .catch((cause: unknown) => setHandoff({ outcome: "panel-unavailable", cause }))
  }, [onboarding.active, handedOver, completed])

  // What the window has to show for the handoff it got, if anything. Kept by
  // the handoff rather than recomputed every render, so a failed close — which
  // changes the note under the buttons — does not take the focus back off the
  // button somebody just pressed.
  const recovery = React.useMemo(() => setupRecovery(handoff), [handoff])

  // An alert dialog nobody is inside is one a screen reader reads past, and
  // setup's own controls have gone with the step that had them: there is
  // nothing for the caret to fall back to but this.
  React.useEffect(() => {
    if (recovery) recoveryDialog.current?.focus()
  }, [recovery])

  // Asking for the handoff again is putting the gate back where it was before
  // the first attempt: the effect above is what performs it, and it runs again
  // because this is the flag it waits on.
  const retryHandoff = React.useCallback(() => {
    setCloseFailed(false)
    setHandoff(undefined)
    setHandedOver(false)
  }, [])

  // Closing without the panel. Every way this can fail is a value rather than a
  // rejection, so the screen can say what happened and keep its buttons; a
  // browser has no window of its own to close, which is not a failure and is
  // also not something this screen can be reached in.
  const closeWindow = React.useCallback(() => {
    void closeSetupWindow().then((closed) => {
      setCloseFailed(closed.outcome !== "closed")
    })
  }, [])

  if (!onboarding.active) {
    // A surface that becomes the panel in place does so now.
    if (children !== undefined) return <>{children}</>
    // The window is closing. The one thing it must not do is go quiet over a
    // handoff that left something behind — a panel that never came up, or this
    // window still standing over one that did. Either way it says which.
    if (recovery) {
      return (
        <HandoffFailed
          ref={recoveryDialog}
          recovery={recovery}
          onRetry={retryHandoff}
          onClose={closeWindow}
          closeFailed={closeFailed}
        />
      )
    }
    return null
  }
  return (
    <div className="nessa-setup-sheet">
      {/*
        What is behind setup dims first, so the box arrives into a settled
        screen rather than competing with the desktop.

        It also takes a click. Setup covers the whole screen, so while it is up
        nothing else on the machine can be reached — and the instinct when a
        window is in the way is to click past it, which until now did nothing at
        all. Clicking away from the box leaves setup, which is what that click
        was asking for.

        Hidden from assistive technology and given no role, because it is not a
        control anyone should be told to look for: Escape and the close button
        are the ways out that announce themselves. This is a courtesy for the
        pointer.
      */}
      <div aria-hidden="true" className="nessa-setup-dim" onClick={onboarding.dismiss} />
      <div
        // Setup covers the whole screen, menu bar included, so it is a modal
        // dialog in fact whether or not it says so. Saying so is what puts a
        // screen reader inside it and names it with the step's own heading.
        role="dialog"
        aria-modal="true"
        aria-labelledby={SETUP_HEADING_ID}
        className="nessa-setup-window"
        data-step={onboarding.state.step}
      >
        {/* The light setup arrives as, inside the panel it will fill. It lives
          in here — clipped by the panel's own bounds — because a form that
          grows until it *is* the window reads as the window being drawn, and
          what should be felt is the window being filled. */}
        <AgentBloom />
        {/* Above the wash rather than inside it, so the way out does not wait
          out the opening's choreography with it. */}
        <SetupChrome onClose={onboarding.dismiss} />
        <Onboarding
          state={onboarding.state}
          accelerator={onboarding.accelerator}
          onBegin={onboarding.begin}
          onChoose={onboarding.choose}
          onConfirm={onboarding.confirm}
          onFinish={onboarding.finish}
          onRecheck={onboarding.recheck}
          checking={onboarding.checking}
          platform={onboarding.platform}
        />
      </div>
    </div>
  )
}
