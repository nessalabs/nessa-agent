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
import type { AgentTurnActivityItem } from "./agent-transcript-view"

const TurnActivityRows = lazy(() => import("./turn-activity-rows"))

export function turnActivitySummary(
  items: readonly AgentTurnActivityItem[],
  running: boolean,
) {
  if (running) return "Running…"
  const tools = items.filter((item) => item.kind === "tool").length
  return tools === 0 ? "Thought" : `Ran ${tools} tool${tools === 1 ? "" : "s"}`
}

export function TurnActivity({
  items,
  running,
  onOpen,
}: {
  items: readonly AgentTurnActivityItem[]
  running: boolean
  onOpen: () => void
}) {
  if (!items.length) return null
  return (
    <AgentActivity status={running ? "running" : "complete"}>
      <AgentActivityTrigger onClick={onOpen}>
        {turnActivitySummary(items, running)}
      </AgentActivityTrigger>
    </AgentActivity>
  )
}

export function TurnActivityDetails({
  items,
  running,
  onClose,
}: {
  items: readonly AgentTurnActivityItem[]
  running: boolean
  onClose: () => void
}) {
  return (
    <Sheet className="nessa-detail-sheet" label="Turn activity" onClose={onClose}>
      <SheetHandle />
      <SheetHeader>
        <SheetExpand />
        <SheetTitle>{turnActivitySummary(items, running)}</SheetTitle>
        <SheetAction>Done</SheetAction>
      </SheetHeader>
      <SheetBody>
        <Suspense fallback={<p>Loading activity details…</p>}>
          <TurnActivityRows items={items} />
        </Suspense>
      </SheetBody>
    </Sheet>
  )
}
