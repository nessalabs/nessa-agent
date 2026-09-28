import {
  memo,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react"
import { shallowEqual } from "react-redux"
import { DesktopIcon } from "../../../ui/icons"
import {
  openBeside,
  openChannel,
  openSession,
  selectStatusView,
  toggleChannel,
  toggleSection,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectChannelIdsIn,
  selectSectionCollapsed,
  selectSections,
  selectSidebarOpen,
  selectStatusCounts,
  selectView,
  selectWaitingIds,
} from "../../adapters/store/selectors"
import { useBesideKey } from "../session-actions"
import { contextMenuFromKey } from "../../adapters/dom/context-menu-key"
import { ColumnHeader } from "../../../ui/column-header"
import { IdentityFooter } from "../chrome/identity-footer"
import { StatusGlyph } from "../chrome/status-glyph"
import { useSidebarPeek, useWorkspaceFrame } from "../workspace-frame"
import { ChannelBranch } from "./channel-branch"
import { ChannelRow, SidebarRow } from "./channel-row"
import "./source-list.css"
import { tooltip } from "../../../ui/tooltip"
import { statusLabels, type AttentionStatus } from "../../model/session-groups"
import {
  AgentsOverviewEntry,
  useAgentsOverviewEnabled,
} from "../../../experiments/agents-overview"

/**
 * The sidebar's source list: sections of channels on a glass pane, with the
 * window's identity at its foot. `channels` is the sidebar beside a session
 * list — "Needs you" and "Running" views, then channels to choose from.
 * `tree` stands alone — search, the next session waiting, then channels
 * that disclose their sessions inline. `top` fills the window's row inside
 * the glass.
 */
export const SourceList = memo(function SourceList({
  variant,
  top,
}: {
  variant: "channels" | "tree"
  top?: ReactNode
}) {
  const open = useWorkspaceSelector(selectSidebarOpen)
  const sections = useWorkspaceSelector(selectSections)
  const treeRef = useRef<HTMLElement>(null)
  const dispatch = useWorkspaceDispatch()
  const besideKey = useBesideKey()
  const peek = useSidebarPeek()
  const revealed = peek !== null && peek.shown && !peek.handingOff

  // Arrow keys walk the tree's visible rows; right and left unfold and fold.
  const onTreeKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    const tree = treeRef.current
    if (!tree || contextMenuFromKey(event)) return
    const rows = [...tree.querySelectorAll<HTMLElement>("[data-row]")]
    const index = rows.indexOf(document.activeElement as HTMLElement)
    if (index < 0) return
    const row = rows[index]
    const move = (to: number) => {
      event.preventDefault()
      rows[Math.max(0, Math.min(rows.length - 1, to))]?.focus()
    }
    if (event.key === "ArrowDown") move(index + 1)
    else if (event.key === "ArrowUp") move(index - 1)
    else if (event.key === "Home") move(0)
    else if (event.key === "End") move(rows.length - 1)
    else if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
      const opening = event.key === "ArrowRight"
      const expanded = row.getAttribute("aria-expanded") === "true"
      const { channel, section, parent } = row.dataset
      if (channel) {
        event.preventDefault()
        if (opening === expanded) move(index + (opening ? 1 : 0))
        else dispatch(toggleChannel({ channelId: channel, open: opening }))
      } else if (section) {
        event.preventDefault()
        if (opening !== expanded) dispatch(toggleSection({ sectionId: section }))
      } else if (!opening && parent) {
        event.preventDefault()
        rows.find((candidate) => candidate.dataset.channel === parent)?.focus()
      }
    } else if (event.key === "Enter" && besideKey.asks(event)) {
      event.preventDefault()
      const { sessionRow, channel } = row.dataset
      if (sessionRow) dispatch(openBeside({ sessionId: sessionRow }))
      else if (channel) dispatch(openChannel({ channelId: channel, beside: true }))
    }
  }

  return (
    <aside
      id="workspace-sidebar"
      className="workspace-sidebar"
      data-variant={variant}
      aria-label="Sidebar"
      // Folded, it is only there while revealed from the window's edge.
      inert={!open && !revealed}
      aria-hidden={(!open && !revealed) || undefined}
      {...(open ? {} : peek?.holders)}
    >
      <ColumnHeader action={top} />
      <div className="workspace-sidebar-scroll">
        {variant === "channels" ? <StatusViews /> : <TreeTop />}
        <nav
          ref={treeRef}
          aria-label="Channels"
          onKeyDown={variant === "tree" ? onTreeKeyDown : undefined}
        >
          {sections.map((section) => (
            <SourceSection
              key={section.id}
              sectionId={section.id}
              name={section.name}
              variant={variant}
            />
          ))}
        </nav>
      </div>
      <IdentityFooter />
    </aside>
  )
})

/**
 * "Needs you" and "Running": every session in that state, in the list beside;
 * or, with the agents overview on, its entry in their place.
 */
function StatusViews() {
  const dispatch = useWorkspaceDispatch()
  const counts = useWorkspaceSelector(selectStatusCounts, shallowEqual)
  const view = useWorkspaceSelector(selectView)
  const showing = (status: AttentionStatus) =>
    view.kind === "status" && view.status === status
  // The agents overview, when on, holds both views: its entry stands in their place.
  if (useAgentsOverviewEnabled())
    return (
      <div className="workspace-smart">
        <AgentsOverviewEntry />
      </div>
    )
  return (
    <div className="workspace-smart">
      <SidebarRow
        icon="needsYou"
        label={statusLabels["needs-you"]}
        active={showing("needs-you")}
        badge={counts.needsYou || undefined}
        badgeTone="needs"
        onClick={() => dispatch(selectStatusView({ status: "needs-you" }))}
      />
      <SidebarRow
        icon="running"
        label={statusLabels.running}
        active={showing("running")}
        badge={counts.running || undefined}
        onClick={() => dispatch(selectStatusView({ status: "running" }))}
      />
    </div>
  )
}

/**
 * Search, which opens the quick switcher, and the way to the next session
 * waiting — or, with the agents overview on, its entry in that place.
 */
function TreeTop() {
  const dispatch = useWorkspaceDispatch()
  const frame = useWorkspaceFrame()
  const waiting = useWorkspaceSelector(selectWaitingIds, shallowEqual)
  // With the agents overview on, its entry is the way to what waits.
  const overviewEnabled = useAgentsOverviewEnabled()
  const shortcut = frame.shortcut("switcher")
  return (
    <>
      <button
        type="button"
        className="workspace-row workspace-search-button"
        onClick={() => frame.openSwitcher("open")}
      >
        <DesktopIcon name="search" />
        <span>Search</span>
        {shortcut ? <kbd className="workspace-kbd">{shortcut}</kbd> : null}
      </button>
      <AgentsOverviewEntry />
      {waiting.length > 0 && !overviewEnabled ? (
        <button
          type="button"
          className="workspace-row workspace-needs-button"
          {...tooltip("Open the next session waiting on you")}
          onClick={() => dispatch(openSession({ sessionId: waiting[0] }))}
        >
          <StatusGlyph status="needs-you" />
          <span>{statusLabels["needs-you"]}</span>
          <span className="workspace-badge" data-tone="needs">
            {waiting.length}
          </span>
        </button>
      ) : null}
    </>
  )
}

const SourceSection = memo(function SourceSection({
  sectionId,
  name,
  variant,
}: {
  sectionId: string
  name: string
  variant: "channels" | "tree"
}) {
  const dispatch = useWorkspaceDispatch()
  const collapsed = useWorkspaceSelector((state) =>
    selectSectionCollapsed(state, sectionId),
  )
  const channelIds = useWorkspaceSelector((state) => selectChannelIdsIn(state, sectionId))
  return (
    <section className="workspace-section" data-collapsed={collapsed || undefined}>
      <div className="workspace-section-head">
        <button
          type="button"
          className="workspace-section-toggle"
          data-row="section"
          data-section={sectionId}
          aria-expanded={!collapsed}
          onClick={() => dispatch(toggleSection({ sectionId }))}
        >
          <span>{name}</span>
          <DesktopIcon name="chevronRight" />
        </button>
        {variant === "channels" ? (
          // Channels cannot be added yet; the control shows where it will be.
          <button
            type="button"
            className="workspace-section-add"
            aria-label={`Add channel to ${name}`}
            {...tooltip("Adding channels isn’t available yet")}
            disabled
          >
            <DesktopIcon name="add" />
          </button>
        ) : null}
      </div>
      {collapsed ? null : (
        <div className="workspace-section-body">
          <ul role="list" className={variant === "tree" ? "workspace-rise" : undefined}>
            {channelIds.map((channelId) =>
              variant === "tree" ? (
                <ChannelBranch key={channelId} channelId={channelId} />
              ) : (
                <ChannelRow key={channelId} channelId={channelId} />
              ),
            )}
          </ul>
        </div>
      )}
    </section>
  )
})
