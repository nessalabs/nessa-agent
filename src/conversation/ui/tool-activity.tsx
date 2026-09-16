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
function summary(tools: Tool[]) {
  const running = tools.some(
    (tool) => tool.status === "pending" || tool.status === "running",
  )
  const failed = tools.filter((tool) => tool.status === "failed").length
  return `${running ? "Running" : "Ran"} ${tools.length} tool${tools.length === 1 ? "" : "s"}${failed ? ` · ${failed} failed` : ""}`
}
export function ToolActivity({ tools, onOpen }: { tools: Tool[]; onOpen: () => void }) {
  if (!tools.length) return null
  const running = tools.some(
    (tool) => tool.status === "pending" || tool.status === "running",
  )
  return (
    <AgentActivity
      status={
        running
          ? "running"
          : tools.some((tool) => tool.status === "failed")
            ? "error"
            : "complete"
      }
    >
      <AgentActivityTrigger onClick={onOpen}>{summary(tools)}</AgentActivityTrigger>
    </AgentActivity>
  )
}
export function ToolDetails({ tools, onClose }: { tools: Tool[]; onClose: () => void }) {
  return (
    <Sheet className="nessa-detail-sheet" label="Tool activity" onClose={onClose}>
      <SheetHandle />
      <SheetHeader>
        <SheetExpand />
        <SheetTitle>{summary(tools)}</SheetTitle>
        <SheetAction>Done</SheetAction>
      </SheetHeader>
      <SheetBody>
        <Suspense fallback={<p>Loading tool details…</p>}>
          <ToolRows tools={tools} />
        </Suspense>
      </SheetBody>
    </Sheet>
  )
}

export function ThoughtActivity({
  running,
  onOpen,
}: {
  running: boolean
  onOpen: () => void
}) {
  return (
    <AgentActivity status={running ? "running" : "complete"}>
      <AgentActivityTrigger onClick={onOpen}>
        {running ? "Thinking" : "Thought"}
      </AgentActivityTrigger>
    </AgentActivity>
  )
}
export function ThoughtDetails({
  thought,
  onClose,
}: {
  thought: string
  onClose: () => void
}) {
  return (
    <Sheet className="nessa-detail-sheet" label="Thought" onClose={onClose}>
      <SheetHandle />
      <SheetHeader>
        <SheetExpand />
        <SheetTitle>Thought</SheetTitle>
        <SheetAction>Done</SheetAction>
      </SheetHeader>
      <SheetBody>
        <p className="whitespace-pre-wrap select-text text-sm">{thought}</p>
      </SheetBody>
    </Sheet>
  )
}
