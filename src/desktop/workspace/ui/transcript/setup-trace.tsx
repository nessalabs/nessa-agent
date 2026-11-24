import { ToolTrace } from "@nessa-ui/react/tool-trace"
import { DesktopIcon } from "../../../ui/icons"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { entry } from "../../application/workspace-state"
import {
  SETUP_WORTH_A_LINE_MS,
  setupReadyLabel,
  setupStepLabel,
  setupTookMs,
  type MachineSetup,
} from "../../model/machine-setup"

/**
 * How long a reply's cloud machine took to get ready, as one folded line
 * above the reply ("Container ready in 39s"), drawn as a tool run is: it
 * opens to each step and how long it took. A machine ready at once says
 * nothing.
 */
export function SetupTrace({
  sessionId,
  setup,
}: {
  sessionId: string
  setup: MachineSetup
}) {
  const agent = useWorkspaceSelector(
    (state) => entry(state.workspace.transcripts, sessionId)?.agent,
  )
  if (setupTookMs(setup) < SETUP_WORTH_A_LINE_MS) return null
  return (
    <ToolTrace
      className="workspace-work"
      data-setup-trace=""
      status="complete"
      summary={setupReadyLabel(setup)}
      steps={setup.steps.map((step, index) => ({
        id: `setup-${index}`,
        label: setupStepLabel(step.step, agent),
        icon: <DesktopIcon name="check" />,
        status: "completed",
        startedAtMs: step.startedAtMs,
        endedAtMs: step.endedAtMs ?? step.startedAtMs,
      }))}
    />
  )
}
