import * as React from "react"
import { host } from "../../host"
import { Onboarding } from "./onboarding"
import { useOnboarding } from "./use-onboarding"

/**
 * Shows first-run setup, and the app only once setup is finished.
 *
 * Setup is its own screen rather than something the panel draws inside itself:
 * it fills the window, owns no panel chrome, and the panel is not mounted
 * behind it. That keeps the panel's frame, tabs and composer out of a surface
 * that has nothing to do with them, and keeps setup out of the panel's own
 * component.
 */
export function SetupGate({ children }: { children: React.ReactNode }) {
  const onboarding = useOnboarding()
  if (!onboarding.active) return <>{children}</>
  // The webview is larger than the window and pinned to its bottom right, so
  // setup takes the same stage and window-sized surface the panel does. It
  // wears none of the panel's chrome: no tabs, no composer, no edge reveal and
  // no resize handle, because none of them belong to a setup screen.
  return (
    <div className="nessa-stage" data-host={host.kind}>
      <div className="nessa-panel relative overflow-hidden">
        <Onboarding
          state={onboarding.state}
          summon={onboarding.summon}
          onBegin={onboarding.begin}
          onChoose={onboarding.choose}
          onConfirm={onboarding.confirm}
          onFinish={onboarding.finish}
          onDismiss={onboarding.dismiss}
        />
      </div>
    </div>
  )
}
