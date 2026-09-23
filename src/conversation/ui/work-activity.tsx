import * as React from "react"
import {
  AgentActivity,
  AgentActivityCue,
  AgentActivityTrigger,
} from "@nessa-ui/react/agent-activity"
import {
  Sheet,
  SheetHandle,
  SheetHeader,
  SheetTitle,
  SheetExpand,
  SheetAction,
  SheetBody,
} from "@nessa-ui/react/sheet"
import { RandomAvatar } from "@nessa-ui/react/random-avatar"
import type { WorkStep } from "./agent-transcript-view"
import { watchSheetGrowth } from "./sheet-growth"

const WorkSteps = React.lazy(() => import("./work-steps"))

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
  if (running) return tools.length ? "Running" : "Thinking"
  if (!tools.length) return work.length ? "Thought" : ""
  const count = `${tools.length} tool${tools.length === 1 ? "" : "s"}`
  // A stopped call did not finish, so the past tense would overstate it.
  const ran = tools.some((tool) => tool.status === "stopped") ? count : `Ran ${count}`
  // What a reader wants first is whether their files were touched. Only kinds
  // the provider actually named are counted: an unsaid kind is `other`, and
  // guessing it as a read would put a number on the line that is not true.
  const read = tools.filter((tool) => tool.kind === "file_read").length
  const wrote = tools.filter((tool) => tool.kind === "file_edit").length
  const bits = [read && `${read} read`, wrote && `${wrote} written`].filter(Boolean)
  return bits.length ? `${ran} · ${bits.join(", ")}` : ran
}
export function WorkActivity({
  work,
  running,
  seed,
  expanded,
  sheetId,
  onOpen,
}: Work & {
  seed: string
  expanded: boolean
  sheetId: string
  onOpen: () => void
}) {
  const label = workSummary({ work, running })
  if (!label) return null
  return (
    <AgentActivity status={running ? "running" : "complete"}>
      {/*
        While it runs the line is a status, not a control: it says the agent
        is working and carries no chevron, because a turn still going has
        nothing settled to go and read. It becomes the count, and a way in,
        when the turn is done.
      */}
      {running ? (
        <AgentActivityCue>{label}</AgentActivityCue>
      ) : (
        <AgentActivityTrigger
          icon={<RandomAvatar seed={seed} className="size-4" />}
          aria-expanded={expanded}
          aria-controls={expanded ? sheetId : undefined}
          onClick={onOpen}
        >
          {label}
        </AgentActivityTrigger>
      )}
    </AgentActivity>
  )
}
/** Grows the sheet rather than jumping it; see `sheet-growth.ts`. */
function useGrowingSheet() {
  const ref = React.useRef<HTMLDivElement>(null)
  React.useEffect(() => {
    const content = ref.current
    const panel = content?.closest('[data-slot="sheet-panel"]')
    if (!content || !(panel instanceof HTMLElement)) return
    return watchSheetGrowth(content, panel)
  }, [])
  return ref
}

export function WorkDetails({
  work,
  running,
  sheetId,
  onClose,
}: Work & { sheetId: string; onClose: () => void }) {
  const growing = useGrowingSheet()
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
        <div ref={growing}>
          <React.Suspense fallback={<p>Loading the details…</p>}>
            <WorkSteps work={work} />
          </React.Suspense>
        </div>
      </SheetBody>
    </Sheet>
  )
}
