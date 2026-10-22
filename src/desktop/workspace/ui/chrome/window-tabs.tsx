/** The window's open documents, above the same split-pane workspace (#1077). */
import { memo, useLayoutEffect, useMemo, useRef, type KeyboardEvent } from "react"
import { panesOf } from "../../../split-panes/model/pane-layout"
import { DesktopIcon } from "../../../ui/icons"
import { IconButton } from "../../../ui/icon-button"
import {
  closePane,
  closeSessionTab,
  focusPane,
  newSession,
  openSession,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectFocusedSessionId,
  selectOverviewOpen,
  selectPanes,
  selectSession,
  selectSessionTabs,
} from "../../adapters/store/selectors"
import { paneItemOf } from "../../model/pane-item"
import { useWorkspaceFrame } from "../workspace-frame"
import { AgentTile } from "./agent-tile"
import "./window-tabs.css"

/** Arrow keys select documents; the existing open/focus commands keep split sizes. */
function moveTab(event: KeyboardEvent<HTMLDivElement>) {
  const keys = ["ArrowLeft", "ArrowRight", "Home", "End"]
  if (!keys.includes(event.key) || event.altKey || event.metaKey || event.ctrlKey) return
  const tabs = [
    ...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]'),
  ]
  const current = tabs.indexOf(event.target as HTMLButtonElement)
  if (current < 0) return
  event.preventDefault()
  event.stopPropagation()
  const next =
    event.key === "Home"
      ? 0
      : event.key === "End"
        ? tabs.length - 1
        : (current + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length
  tabs[next]?.focus()
  tabs[next]?.click()
}

export const WindowTabs = memo(function WindowTabs() {
  const dispatch = useWorkspaceDispatch()
  const panes = useWorkspaceSelector(selectPanes)
  const ids = useWorkspaceSelector(selectSessionTabs)
  const overview = useWorkspaceSelector(selectOverviewOpen)
  const focused = useWorkspaceSelector(selectFocusedSessionId)
  const widgets = useMemo(
    () =>
      panes ? panesOf(panes).filter((p) => paneItemOf(p.item)?.kind === "widget") : [],
    [panes],
  )
  const list = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({
      block: "nearest",
      inline: "nearest",
    })
  }, [panes?.focused, focused, overview])
  const frame = useWorkspaceFrame()
  return (
    <div className="workspace-window-tabs" data-tauri-drag-region>
      <div
        className="workspace-tab-list"
        ref={list}
        role="tablist"
        aria-label="Open views"
        onKeyDown={moveTab}
      >
        {ids.map((id) => (
          <SessionTab key={id} sessionId={id} />
        ))}
        {widgets.map((pane) => (
          <div
            key={pane.key}
            className="workspace-window-tab"
            data-active={(!overview && panes?.focused === pane.key) || undefined}
          >
            <button
              type="button"
              role="tab"
              aria-selected={!overview && panes?.focused === pane.key}
              tabIndex={!overview && panes?.focused === pane.key ? 0 : -1}
              onClick={() => dispatch(focusPane({ pane: pane.key }))}
            >
              <DesktopIcon name="panelRight" size={16} />
              <span>Widget</span>
            </button>
            <IconButton
              icon="close"
              size="sm"
              label="Close Widget Tab"
              tabIndex={-1}
              onClick={() => dispatch(closePane({ pane: pane.key }))}
            />
          </div>
        ))}
      </div>
      <IconButton
        icon="add"
        label="New Tab"
        shortcut={frame.shortcut("newSession")}
        onClick={() => dispatch(newSession())}
      />
      <div className="workspace-tabs-space" data-tauri-drag-region />
      <IconButton
        icon="splitRight"
        label="Split Right"
        shortcut={frame.shortcut("splitRight")}
        onClick={() => dispatch(newSession({ beside: "right" }))}
      />
    </div>
  )
})

function SessionTab({ sessionId }: { sessionId: string }) {
  const dispatch = useWorkspaceDispatch()
  const session = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const active = useWorkspaceSelector(
    (state) => !selectOverviewOpen(state) && selectFocusedSessionId(state) === sessionId,
  )
  const title = session?.title ?? "New session"
  return (
    <div
      className="workspace-window-tab"
      data-active={active || undefined}
      data-session-tab={sessionId}
    >
      <button
        type="button"
        role="tab"
        aria-selected={active}
        tabIndex={active ? 0 : -1}
        title={title}
        onClick={() => dispatch(openSession({ sessionId }))}
      >
        {session ? (
          <AgentTile model={session.model} size={16} />
        ) : (
          <DesktopIcon name="newSession" size={16} />
        )}
        <span>{title}</span>
      </button>
      <IconButton
        icon="close"
        size="sm"
        label={`Close Tab: ${title}`}
        tabIndex={-1}
        onClick={() => dispatch(closeSessionTab({ sessionId }))}
      />
    </div>
  )
}
