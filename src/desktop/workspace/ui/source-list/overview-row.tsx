import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { showContent } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectOverviewOpen, selectStatusCounts } from "../../adapters/store/selectors"
import { useWorkspaceFrame } from "../workspace-frame"

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
    <button
      type="button"
      className="workspace-row agents-overview-entry"
      data-active={open || undefined}
      aria-current={open ? "page" : undefined}
      {...tooltip("Every agent at a glance", {
        shortcut: frame.shortcut("showOverview"),
      })}
      onClick={() => dispatch(showContent({ content: "agents" }))}
    >
      <span className="workspace-row-icon" aria-hidden="true">
        <DesktopIcon name="workspace" />
      </span>
      <span className="workspace-truncate">Agents</span>
      {waiting > 0 ? (
        <span
          className="workspace-badge"
          data-tone="needs"
          aria-label={`${waiting} ${waiting === 1 ? "needs" : "need"} you`}
        >
          {waiting}
        </span>
      ) : null}
    </button>
  )
}
