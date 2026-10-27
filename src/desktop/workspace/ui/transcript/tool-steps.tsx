import { useState } from "react"
import { ToolTrace, type ToolTraceStep } from "@nessa-ui/react/tool-trace"
import { DesktopIcon, type DesktopIconRole } from "../../../ui/icons"
import type { StepKind, StepPart } from "../../model/transcript"
import { WithChats } from "./chat-pill"
import { useSiteIcon } from "./site-icon"

const stepIcons: Record<StepKind, DesktopIconRole> = {
  read: "file",
  edit: "edit",
  run: "terminal",
  search: "search",
}

/**
 * What the agent did, as the kit's waterfall (`ToolTrace`): one line with a
 * bar of the whole run that opens to each call on a time axis, and a call's
 * card to its diff, input and output. While the turn runs the trace is open
 * and keeps time; once the turn has ended (`settled`) it folds to its line.
 */
export function ToolSteps({
  steps,
  settled = false,
}: {
  steps: readonly StepPart[]
  /** The turn this run belongs to has ended (`TurnMessage.working` is absent). */
  settled?: boolean
}) {
  const [open, setOpen] = useState(!settled)
  // The run folds once, when its turn ends; after that the person decides.
  const [wasSettled, setWasSettled] = useState(settled)
  if (settled !== wasSettled) {
    setWasSettled(settled)
    if (settled) setOpen(false)
  }
  return (
    <ToolTrace
      className="workspace-work"
      status={settled ? "complete" : "running"}
      open={open}
      onOpenChange={setOpen}
      steps={steps.map((step, index) => traceStep(step, index, steps.length, settled))}
      // A chat a tool named is its pill, drawn still: the row is the button.
      renderText={chatsIn}
    />
  )
}

const chatsIn = (text: string) => <WithChats text={text} still />

/**
 * A step as the trace draws it. A step that says nothing of its status is
 * finished once its turn is; while the turn runs, the last such step is the
 * one at work.
 */
function traceStep(
  step: StepPart,
  index: number,
  count: number,
  settled: boolean,
): ToolTraceStep {
  const status =
    step.status ?? (!settled && index === count - 1 ? "running" : "completed")
  return {
    id: step.id ?? `step-${index}`,
    label: step.label,
    ...(step.detail ? { detail: step.detail } : {}),
    icon: <StepIcon step={step} />,
    status,
    strong: step.step === "run" || step.step === "edit",
    ...(step.startedAtMs !== undefined ? { startedAtMs: step.startedAtMs } : {}),
    ...(step.endedAtMs !== undefined ? { endedAtMs: step.endedAtMs } : {}),
    ...(step.added !== undefined ? { added: step.added } : {}),
    ...(step.removed !== undefined ? { removed: step.removed } : {}),
    ...(step.diff ? { diff: step.diff } : {}),
    ...(step.input !== undefined ? { input: step.input } : {}),
    ...(step.output !== undefined ? { output: step.output } : {}),
  }
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
