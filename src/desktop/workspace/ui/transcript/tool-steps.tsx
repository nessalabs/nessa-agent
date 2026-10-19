import { useId, useState } from "react"
import { AgentActivity, AgentActivityTrigger } from "@nessa-ui/react/agent-activity"
import { DesktopIcon, type DesktopIconRole } from "../../../ui/icons"
import { workSummary, type StepKind, type StepPart } from "../../model/transcript"
import { useSiteIcon } from "./site-icon"

const stepIcons: Record<StepKind, DesktopIconRole> = {
  read: "file",
  edit: "edit",
  run: "terminal",
  search: "search",
}

/**
 * What the agent did. While its turn runs, one quiet line each, on a
 * hairline, so the reader follows the work as it happens. Once the turn has
 * ended (`settled`), the run is one line that says what it came to
 * (`workSummary`) and opens to the same steps in place.
 */
export function ToolSteps({
  steps,
  settled = false,
}: {
  steps: readonly StepPart[]
  /** The turn this run belongs to has ended (`TurnMessage.working` is absent). */
  settled?: boolean
}) {
  const [open, setOpen] = useState(false)
  const listId = useId()
  if (!settled) return <StepList steps={steps} />
  return (
    <AgentActivity className="workspace-work" status="complete">
      <AgentActivityTrigger
        // It opens the steps below it, not a sheet.
        opens="inline"
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        onClick={() => setOpen((shown) => !shown)}
      >
        {workSummary(steps)}
      </AgentActivityTrigger>
      {open ? <StepList id={listId} steps={steps} /> : null}
    </AgentActivity>
  )
}

function StepList({ id, steps }: { id?: string; steps: readonly StepPart[] }) {
  return (
    <ul id={id} className="workspace-steps">
      {steps.map((step, index) => (
        <li key={index}>
          <StepIcon step={step} />
          <span className="workspace-step-label">{step.label}</span>
          {step.detail ? (
            <span className="workspace-step-detail">{step.detail}</span>
          ) : null}
          {step.added !== undefined ? (
            <span className="workspace-diff">
              <ins>+{step.added}</ins>
              {step.removed ? <del>−{step.removed}</del> : null}
            </span>
          ) : null}
        </li>
      ))}
    </ul>
  )
}

/**
 * A step's icon: its kind's, or — for a step on a web page, such as a fetch
 * — that site's own icon once it has loaded, the kind's standing in until
 * then and wherever the site has none. The site's icon comes from the
 * composed `SiteIcon`; without one, the kind's icon is all there is.
 */
function StepIcon({ step }: { step: StepPart }) {
  const siteIcon = useSiteIcon()
  const icon = <DesktopIcon name={stepIcons[step.step]} />
  return siteIcon && step.detail && /^https?:\/\//i.test(step.detail)
    ? siteIcon({ href: step.detail, fallback: icon })
    : icon
}
