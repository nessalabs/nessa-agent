import { SetupGate } from "../onboarding"
import type { AppDependencies } from "./dependencies"
import type { ReactNode } from "react"

/**
 * First-run setup for a surface that has no host, wired to the dependencies
 * that have to hear what it decided.
 *
 * Its own component, and its own file, because it is a join rather than a
 * piece of UI: the gate records the agent and the panel names it on every
 * creation, and in a browser nothing else connects the two — there is no host
 * to write the choice to and no second window to read it back in. Deleting the
 * connection leaves both halves working and tested and the picker deciding
 * nothing, which is the failure this exists to make impossible to reintroduce
 * quietly. Small enough to mount in a test without the panel behind it.
 */
export function BrowserSetupGate({
  dependencies,
  children,
}: {
  dependencies: Pick<AppDependencies, "agents" | "rememberChosenAgent">
  /** The panel this surface becomes once setup is over. */
  children: ReactNode
}) {
  return (
    <SetupGate agents={dependencies.agents} onHandOver={dependencies.rememberChosenAgent}>
      {children}
    </SetupGate>
  )
}
