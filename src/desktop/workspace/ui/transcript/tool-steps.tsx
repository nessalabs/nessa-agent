import { DesktopIcon, type DesktopIconRole } from "../../../ui/icons"
import type { StepKind, StepPart } from "../../model/transcript"

const stepIcons: Record<StepKind, DesktopIconRole> = {
  read: "file",
  edit: "edit",
  run: "terminal",
  search: "search",
}

/** What the agent did, one quiet line each, on a hairline. */
export function ToolSteps({ steps }: { steps: readonly StepPart[] }) {
  return (
    <ul className="workspace-steps">
      {steps.map((step, index) => (
        <li key={index}>
          <DesktopIcon name={stepIcons[step.step]} />
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
