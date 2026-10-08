import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { showContent } from "../../adapters/store/commands"
import { selectOverviewOpen, selectStatusCounts } from "../../adapters/store/selectors"
import { useWorkspaceFrame } from "../workspace-frame"
import { SidebarRow } from "./sidebar-row"

/**
 * The sidebar's way into the Agents overview: "Agents", with how many wait
 * on the person — the overview holds what waits and what runs. A place to
 * go, like a channel: chosen again, it stays; ⌘0 does the same.
 */
export function OverviewRow() {
  const dispatch = useWorkspaceDispatch()
  const frame = useWorkspaceFrame()
  const open = useWorkspaceSelector(selectOverviewOpen)
  const waiting = useWorkspaceSelector((state) => selectStatusCounts(state).needsYou)
  return (
    <SidebarRow
      className="agents-overview-entry"
      icon="workspace"
      label="Agents"
      title="Every agent at a glance"
      shortcut={frame.shortcut("showOverview")}
      active={open}
      badge={waiting}
      badgeTone="needs"
      badgeLabel={`${waiting} ${waiting === 1 ? "needs" : "need"} you`}
      onClick={() => dispatch(showContent({ content: "agents" }))}
    />
  )
}
