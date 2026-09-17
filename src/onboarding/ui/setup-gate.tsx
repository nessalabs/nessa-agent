import * as React from "react"
import {
  closeSetupWindow,
  finishSetupWindow,
  recordSetupComplete,
  revealSetupWindow,
  type SetupHandoff,
} from "../../host"
import { AgentBloom } from "./agent-bloom"
import { Onboarding, SETUP_HEADING_ID } from "./onboarding"
import { SetupChrome } from "./setup-chrome"
import { useIntroSound } from "./use-intro-sound"
import { useOnboarding } from "./use-onboarding"
import type { AgentReadinessSource } from "../application/ports"
import { recordsSetupCompletion } from "../application/setup-completion"

/** The buttons on the handoff-failure screen, which are the whole point of it:
 * a window with nothing in it but a sentence is a window someone has to kill. */
const RECOVERY_BUTTON =
  "h-10 rounded-full border border-border bg-card px-5 nessa-text-3 font-medium text-foreground transition-colors outline-none hover:bg-accent hover:text-accent-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/40"

/**
 * What is on screen when setup finished and the panel did not come up.
 *
 * Both ways out are here, because the two failures underneath are different:
 * the panel may come up on a second try, and if it does not, this window still
 * has to be got rid of without hunting for a menu bar behind it.
 *
 * A close that itself fails changes nothing but the note. The screen stays, with
 * both buttons still on it, rather than throwing out of an event handler and
 * taking the last controls with it.
 */
export function HandoffFailed({
  onRetry,
  onClose,
  closeFailed,
}: {
  onRetry: () => void
  onClose: () => void
  /** True once a close was asked for and the window system did not do it. */
  closeFailed: boolean
}) {
  return (
    <div className="nessa-setup-sheet">
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={SETUP_HEADING_ID}
        className="nessa-setup-window nessa-setup-light flex flex-col items-center justify-center gap-4 overflow-y-auto bg-background p-8 text-center text-foreground"
      >
        <h1 id={SETUP_HEADING_ID} className="nessa-text-6 font-semibold">
          Nessa could not open the panel
        </h1>
        <p className="nessa-text-3 text-muted-foreground">
          Setup finished. Try again, or summon Nessa from the menu bar icon or with your
          summon shortcut.
        </p>
        <div className="flex flex-wrap items-center justify-center gap-3">
          <button type="button" className={RECOVERY_BUTTON} onClick={onRetry}>
            Try again
          </button>
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
            This window would not close. Close it from the window controls.
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

  useIntroSound(onboarding.active)

  // The window is created hidden and shown from here, after this has rendered
  // — so the first thing on screen is the opening rather than an empty window
  // waiting for its first frame.
  React.useEffect(() => {
    const shown = requestAnimationFrame(() => void revealSetupWindow())
    return () => cancelAnimationFrame(shown)
  }, [])

  // Handing over to the panel, and — only if that lands — writing setup off for
  // good. The order matters and used to be the other way round: the flag was
  // persisted the moment somebody pressed the last button, so a panel that
  // failed to come up left them on the recovery screen with first-run setup
  // already marked done forever on a machine they had never seen it work on.
  // The rule about which endings count is `recordsSetupCompletion`.
  const setup = onboarding.state
  React.useEffect(() => {
    if (onboarding.active || handedOver) return
    setHandedOver(true)
    void finishSetupWindow()
      .then((landed) => {
        setHandoff(landed)
        if (!recordsSetupCompletion(setup, landed.outcome)) return
        // Not awaited, and its failure does not travel: the handoff is the
        // thing somebody is waiting on, and a settings file that would not take
        // the flag costs them a second run of setup, not their panel.
        void recordSetupComplete().catch((cause: unknown) => {
          console.warn("[nessa] could not record that setup finished", cause)
        })
      })
      .catch((cause: unknown) => setHandoff({ outcome: "panel-unavailable", cause }))
  }, [onboarding.active, handedOver, setup])

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
    // The window is closing. The one thing it must not do is close silently
    // over a panel that never came up, so a failed handoff is said out loud
    // rather than leaving a blank window nobody can explain.
    if (handoff?.outcome === "panel-unavailable") {
      return (
        <HandoffFailed
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
