import * as React from "react"
import { finishSetupWindow, revealSetupWindow, type SetupHandoff } from "../../host"
import { AgentBloom } from "./agent-bloom"
import { Onboarding, SETUP_HEADING_ID } from "./onboarding"
import { SetupChrome } from "./setup-chrome"
import { useIntroSound } from "./use-intro-sound"
import { useOnboarding } from "./use-onboarding"
import type { AgentReadinessSource } from "../application/ports"

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

  useIntroSound(onboarding.active)

  // The window is created hidden and shown from here, after this has rendered
  // — so the first thing on screen is the opening rather than an empty window
  // waiting for its first frame.
  React.useEffect(() => {
    const shown = requestAnimationFrame(() => void revealSetupWindow())
    return () => cancelAnimationFrame(shown)
  }, [])

  React.useEffect(() => {
    if (onboarding.active || handedOver) return
    setHandedOver(true)
    void finishSetupWindow()
      .then(setHandoff)
      .catch((cause: unknown) => setHandoff({ outcome: "panel-unavailable", cause }))
  }, [onboarding.active, handedOver])

  if (!onboarding.active) {
    // A surface that becomes the panel in place does so now.
    if (children !== undefined) return <>{children}</>
    // The window is closing. The one thing it must not do is close silently
    // over a panel that never came up, so a failed handoff is said out loud
    // rather than leaving a blank window nobody can explain.
    if (handoff?.outcome === "panel-unavailable") {
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
              Setup finished. Summon Nessa from the menu bar icon, or with your summon
              shortcut.
            </p>
          </div>
        </div>
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
          platform={onboarding.platform}
        />
      </div>
    </div>
  )
}
