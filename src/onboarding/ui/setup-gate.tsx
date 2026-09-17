import * as React from "react"
import { finishSetupWindow } from "../../host"
import { AgentBloom } from "./agent-bloom"
import { Onboarding } from "./onboarding"
import { useIntroSound } from "./use-intro-sound"
import { useOnboarding } from "./use-onboarding"

/**
 * The setup surface: first run in its own window.
 *
 * The desktop host opens this in a small centred window of its own, so setup
 * never borrows the panel's frame, tabs or composer, and the panel is not
 * mounted behind it. Finishing or skipping shows the panel and closes this
 * window. In a plain browser there is no second window, so the same component
 * hands over to `children` in place.
 */
export function SetupGate({ children }: { children: React.ReactNode }) {
  const onboarding = useOnboarding()
  const [handedOver, setHandedOver] = React.useState(false)

  useIntroSound(onboarding.active)

  React.useEffect(() => {
    if (onboarding.active || handedOver) return
    setHandedOver(true)
    void finishSetupWindow()
  }, [onboarding.active, handedOver])

  if (!onboarding.active) return <>{children}</>
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
      <div className="nessa-setup-window" data-step={onboarding.state.step}>
        {/* The light setup arrives as, inside the panel it will fill. It lives
          in here — clipped by the panel's own bounds — because a form that
          grows until it *is* the window reads as the window being drawn, and
          what should be felt is the window being filled. */}
        <AgentBloom />
        <Onboarding
          state={onboarding.state}
          accelerator={onboarding.accelerator}
          onBegin={onboarding.begin}
          onChoose={onboarding.choose}
          onConfirm={onboarding.confirm}
          onFinish={onboarding.finish}
          onDismiss={onboarding.dismiss}
          platform={onboarding.platform}
        />
      </div>
    </div>
  )
}
