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
import type { AgentToolView } from "./agent-transcript-view"

const ToolRows = lazy(() => import("./tool-rows"))

type Tool = AgentToolView
export type Work = { thought?: string; tools?: Tool[] }

function isRunning(tools: Tool[]) {
  return tools.some((tool) => tool.status === "pending" || tool.status === "running")
}
/**
 * What a turn's working is called when it is one line.
 *
 * A tool that failed is not in it. An agent trying something that does not
 * work and then trying something else is how agents work; counting those in
 * the transcript tells a reader something is wrong with their request when
 * nothing is. The failure is in the expansion, for whoever goes looking. A
 * turn that failed is said by the conversation notice, which is not this.
 */
export function workSummary({ thought, tools = [] }: Work) {
  if (!tools.length) return thought ? "Thought" : ""
  if (isRunning(tools)) return "Running…"
  return `Ran ${tools.length} tool${tools.length === 1 ? "" : "s"}`
}
export function WorkActivity({ work, onOpen }: { work: Work; onOpen: () => void }) {
  const label = workSummary(work)
  if (!label) return null
  return (
    <AgentActivity status={isRunning(work.tools ?? []) ? "running" : "complete"}>
      <AgentActivityTrigger onClick={onOpen}>{label}</AgentActivityTrigger>
    </AgentActivity>
  )
}
export function WorkDetails({ work, onClose }: { work: Work; onClose: () => void }) {
  const label = workSummary(work)
  return (
    <Sheet className="nessa-detail-sheet" label={label} onClose={onClose}>
      <SheetHandle />
      <SheetHeader>
        <SheetExpand />
        <SheetTitle>{label}</SheetTitle>
        <SheetAction>Done</SheetAction>
      </SheetHeader>
      <SheetBody>
        {work.thought ? (
          <p className="whitespace-pre-wrap select-text text-sm">{work.thought}</p>
        ) : null}
        {work.tools?.length ? (
          <Suspense fallback={<p>Loading tool details…</p>}>
            <ToolRows tools={work.tools} />
          </Suspense>
        ) : null}
      </SheetBody>
    </Sheet>
  )
}
