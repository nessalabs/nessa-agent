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
      {/* What is behind setup dims first, so the box arrives into a settled
        screen rather than competing with the desktop. Decorative: the dim is
        not a control and closing setup is the corner button's job. */}
      <div aria-hidden="true" className="nessa-setup-dim" />
      {/* The agent's colours, living on the dimmed screen for a beat before the
        box opens out of them. It sits outside the box because for most of its
        life there is no box to sit in. */}
      <AgentBloom />
      <div className="nessa-setup-window" data-step={onboarding.state.step}>
        <Onboarding
          state={onboarding.state}
          accelerator={onboarding.accelerator}
          onBegin={onboarding.begin}
          onChoose={onboarding.choose}
          onConfirm={onboarding.confirm}
          onFinish={onboarding.finish}
          platform={onboarding.platform}
          onConfirmSummon={onboarding.confirmSummon}
        />
      </div>
    </div>
  )
}
