import { useEffect, useState } from "react"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { showContent } from "../../adapters/store/commands"
import { selectOverviewOpen, selectStatusCounts } from "../../adapters/store/selectors"
import { useWorkspaceFrame } from "../workspace-frame"
import { SidebarRow } from "./channel-row"

/** The sidebar's overview entry publishes its selected paint after opening. */
export function OverviewRow() {
  const dispatch = useWorkspaceDispatch()
  const frame = useWorkspaceFrame()
  const open = useWorkspaceSelector(selectOverviewOpen)
  const [marked, setMarked] = useState(false)
  useEffect(() => {
    if (!open) {
      setMarked(false)
      return
    }
    let active = true
    let inner: number | null = null
    const outer = requestAnimationFrame(() => {
      if (!active) return
      inner = requestAnimationFrame(() => {
        if (active) setMarked(true)
      })
    })
    return () => {
      active = false
      cancelAnimationFrame(outer)
      if (inner !== null) cancelAnimationFrame(inner)
    }
  }, [open])
  const waiting = useWorkspaceSelector((state) => selectStatusCounts(state).needsYou)
  return (
    <SidebarRow
      className="agents-overview-entry"
      icon="workspace"
      label="Agents"
      title="Every agent at a glance"
      shortcut={frame.shortcut("showOverview")}
      active={marked}
      current={marked}
      badge={waiting}
      badgeTone="needs"
      badgeLabel={`${waiting} ${waiting === 1 ? "needs" : "need"} you`}
      onClick={() => dispatch(showContent({ content: "agents" }))}
    />
  )
}
