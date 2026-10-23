/** The window's open documents, above the same split-pane workspace (#1077). */
import {
  Fragment,
  memo,
  useEffect,
  useLayoutEffect,
  useRef,
  type CSSProperties,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react"
import { panesOf, type PaneLayout } from "../../../split-panes/model/pane-layout"
import { DesktopIcon } from "../../../ui/icons"
import { IconButton } from "../../../ui/icon-button"
import {
  closePane,
  closeSessionTab,
  focusPane,
  newSession,
  openSession,
  splitOffTab,
  switchTab,
} from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectFocusedSessionId,
  selectOverviewOpen,
  selectPanes,
  selectSession,
  selectTabs,
} from "../../adapters/store/selectors"
import { paneItemKey, paneItemOf, sessionItem } from "../../model/pane-item"
import { layoutOf, type WindowTabs as TabsState } from "../../model/window-tabs"
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
  const tabs = useWorkspaceSelector(selectTabs)
  const overview = useWorkspaceSelector(selectOverviewOpen)
  const focused = useWorkspaceSelector(selectFocusedSessionId)
  const list = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({
      block: "nearest",
      inline: "nearest",
    })
  }, [panes?.focused, focused, overview, tabs.activeTab])
  const frame = useWorkspaceFrame()
  return (
    <div className="workspace-window-tabs" data-tauri-drag-region>
      <div
        className="workspace-tab-list"
        ref={list}
        role="tablist"
        aria-label="Open views"
        // Its natural width: every chat a unit, every merged group its glyph.
        style={stripMeasure(tabs, panes)}
        onKeyDown={moveTab}
      >
        {tabs.tabs.map((tab) => {
          const active = !overview && tab.id === tabs.activeTab
          const layout = layoutOf(tabs, panes, tab)
          const shown = layout ? panesOf(layout) : []
          const lit = (key: number) => active && layout?.focused === key
          const members = shown.map((pane) => (
            <TabItem
              key={pane.item}
              tabId={tab.id}
              item={pane.item}
              paneKey={pane.key}
              active={lit(pane.key)}
              merged={shown.length > 1}
            />
          ))
          if (shown.length < 2) return <Fragment key={tab.id}>{members}</Fragment>
          // A merged tab: the chats it shows side by side travel together.
          return (
            <div
              key={tab.id}
              className="workspace-tab-group"
              role="group"
              data-tab={tab.id}
              data-active={active || undefined}
              style={{ "--tab-chats": shown.length } as CSSProperties}
              aria-label={`${shown.length} chats side by side`}
              title={`${shown.length} chats side by side`}
            >
              <MergedGlyph />
              {members}
            </div>
          )
        })}
      </div>
      <IconButton
        icon="add"
        label="New Tab"
        shortcut={frame.shortcut("newSession")}
        onClick={() => dispatch(newSession())}
      />
      {/* Splitting is a drag, ⌘D or a pane's menu: no button for it here. */}
      <div className="workspace-tabs-space" data-tauri-drag-region />
    </div>
  )
})

/** One item of a tab: a chat, or a widget a pane of it shows. */
function TabItem({
  tabId,
  item,
  paneKey,
  active,
  merged,
}: {
  tabId: string
  item: string
  paneKey: number
  active: boolean
  merged: boolean
}) {
  const dispatch = useWorkspaceDispatch()
  const shown = paneItemOf(item)
  if (shown?.kind === "session")
    return <SessionTab sessionId={shown.sessionId} active={active} merged={merged} />
  return (
    <div className="workspace-window-tab" data-active={active || undefined}>
      <button
        type="button"
        role="tab"
        aria-selected={active}
        tabIndex={active ? 0 : -1}
        onClick={() => {
          dispatch(switchTab({ tabId }))
          dispatch(focusPane({ pane: paneKey }))
        }}
      >
        <DesktopIcon name="panelRight" size={16} />
        <span>Widget</span>
      </button>
      <IconButton
        icon="close"
        size="sm"
        label="Close Widget Tab"
        tabIndex={-1}
        onClick={() => {
          dispatch(switchTab({ tabId }))
          dispatch(closePane({ pane: paneKey }))
        }}
      />
    </div>
  )
}

/**
 * Pulled out of its merged tab onto the strip, a chat stands as a tab of its
 * own. Dropped on a pane instead, the panes' own drag merges it there
 * (`data-drag-item`), so this only answers a release over the strip.
 */
function useDragOut(sessionId: string, merged: boolean) {
  const dispatch = useWorkspaceDispatch()
  const stop = useRef<(() => void) | null>(null)
  useEffect(() => () => stop.current?.(), [])
  return (event: ReactPointerEvent<HTMLElement>) => {
    stop.current?.()
    if (!merged || event.button !== 0) return
    const group = event.currentTarget.closest(".workspace-tab-group")
    const { pointerId } = event
    const from = { x: event.clientX, y: event.clientY }
    const end = () => {
      window.removeEventListener("pointerup", up, true)
      window.removeEventListener("pointercancel", end, true)
      window.removeEventListener("blur", end)
      stop.current = null
    }
    const up = (lift: PointerEvent) => {
      if (lift.pointerId !== pointerId) return
      end()
      if (Math.hypot(lift.clientX - from.x, lift.clientY - from.y) < 8) return
      const under = document.elementFromPoint(lift.clientX, lift.clientY)
      if (!under?.closest(".workspace-window-tabs") || (group && group.contains(under)))
        return
      dispatch(splitOffTab({ sessionId }))
    }
    window.addEventListener("pointerup", up, true)
    window.addEventListener("pointercancel", end, true)
    window.addEventListener("blur", end)
    stop.current = end
  }
}

function SessionTab({
  sessionId,
  active,
  merged,
}: {
  sessionId: string
  active: boolean
  merged: boolean
}) {
  const dispatch = useWorkspaceDispatch()
  const session = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const title = session?.title ?? "New session"
  const onPointerDown = useDragOut(sessionId, merged)
  return (
    <div
      className="workspace-window-tab"
      data-active={active || undefined}
      data-session-tab={sessionId}
      onPointerDown={onPointerDown}
    >
      <button
        type="button"
        // Carried onto a pane's side, the panes' drag merges it there.
        data-drag-item={session ? paneItemKey(sessionItem(sessionId)) : undefined}
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

/** Two lines meeting in one: the chats in this group share the window. */
function MergedGlyph() {
  return (
    <svg
      className="workspace-tab-group-glyph"
      width="14"
      height="14"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <circle cx="4" cy="3.5" r="1.75" />
      <circle cx="4" cy="12.5" r="1.75" />
      <circle cx="12" cy="8" r="1.75" />
      <path d="M4 5.25v5.5" />
      <path d="M4 5.25c0 2.75 2.5 2.75 6.25 2.75" />
    </svg>
  )
}

/** The strip's natural width, as units of tab and merged groups (`window-tabs.css`). */
function stripMeasure(tabs: TabsState, panes: PaneLayout | null): CSSProperties {
  let units = 0
  let groups = 0
  for (const tab of tabs.tabs) {
    const layout = layoutOf(tabs, panes, tab)
    const count = layout ? panesOf(layout).length : 0
    units += Math.max(1, count)
    if (count > 1) groups += 1
  }
  return {
    "--tab-units": units,
    "--tab-groups": groups,
    "--tab-count": tabs.tabs.length,
  } as CSSProperties
}
