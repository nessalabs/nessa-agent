import * as React from "react"
import { finishSetupWindow } from "../../host"
import { Onboarding } from "./onboarding"
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

  React.useEffect(() => {
    if (onboarding.active || handedOver) return
    setHandedOver(true)
    void finishSetupWindow()
  }, [onboarding.active, handedOver])

  if (!onboarding.active) return <>{children}</>
  return (
    <div className="nessa-setup-window" data-step={onboarding.state.step}>
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
  )
}
