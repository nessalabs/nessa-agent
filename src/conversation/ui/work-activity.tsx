import { lazy, Suspense } from "react"
import { AgentActivity, AgentActivityTrigger } from "@nessa-ui/react/agent-activity"
import {
  Sheet,
  SheetHandle,
  SheetHeader,
  SheetTitle,
  SheetExpand,
  SheetAction,
  SheetBody,
} from "@nessa-ui/react/sheet"
import type { WorkStep } from "./agent-transcript-view"

const WorkSteps = lazy(() => import("./work-steps"))

export type Work = { work: WorkStep[]; running: boolean }

/**
 * What a turn's working is called when it is one line.
 *
 * A tool that failed is not in it. An agent trying something that does not
 * work and then trying something else is how agents work; counting those in
 * the transcript tells a reader something is wrong with their request when
 * nothing is. The failure is in the expansion, for whoever goes looking. A
 * turn that failed is said by the conversation notice, which is not this.
 */
export function workSummary({ work, running }: Work) {
  const tools = work.flatMap((step) => (step.tool ? [step.tool] : []))
  if (running) return tools.length ? "Running…" : "Thinking"
  if (!tools.length) return work.length ? "Thought" : ""
  const count = `${tools.length} tool${tools.length === 1 ? "" : "s"}`
  // A stopped call did not finish, so the past tense would overstate it.
  return tools.some((tool) => tool.status === "stopped") ? count : `Ran ${count}`
}
export function WorkActivity({
  work,
  running,
  expanded,
  sheetId,
  onOpen,
}: Work & { expanded: boolean; sheetId: string; onOpen: () => void }) {
  const label = workSummary({ work, running })
  if (!label) return null
  return (
    <AgentActivity status={running ? "running" : "complete"}>
      <AgentActivityTrigger
        aria-expanded={expanded}
        aria-controls={expanded ? sheetId : undefined}
        onClick={onOpen}
      >
        {label}
      </AgentActivityTrigger>
    </AgentActivity>
  )
}
export function WorkDetails({
  work,
  running,
  sheetId,
  onClose,
}: Work & { sheetId: string; onClose: () => void }) {
  return (
    // The name stays put while the status moves, so the sheet does not rename
    // itself under a screen reader every time a tool settles.
    <Sheet
      id={sheetId}
      className="nessa-detail-sheet"
      label="Agent working"
      onClose={onClose}
    >
      <SheetHandle />
      <SheetHeader>
        <SheetExpand />
        <SheetTitle>{workSummary({ work, running })}</SheetTitle>
        <SheetAction>Done</SheetAction>
      </SheetHeader>
      <SheetBody>
        <Suspense fallback={<p>Loading the details…</p>}>
          <WorkSteps work={work} />
        </Suspense>
      </SheetBody>
    </Sheet>
  )
}
