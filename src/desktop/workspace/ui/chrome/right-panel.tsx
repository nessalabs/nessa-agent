/**
 * The right panel: a column beside the panes, in both layouts, with its
 * toggle and maximize button at the titlebar's end (⌥⌘B shows and hides it).
 * Its edge resizes it, and dragged far enough right folds it. Maximized, it
 * covers the workspace — the sidebar, the list and the panes are inert under
 * it (`Covered`, `workspace-layout.tsx`) and the window's keys rest but ⌥⌘B —
 * until Escape or Restore. Until something opens in it, it says so.
 */
import { memo, useLayoutEffect, useRef } from "react"
import { EmptyState } from "@nessa-ui/react/empty-state"
import { PANEL_MAX_WIDTH, PANEL_MIN_WIDTH } from "../../../model/right-panel"
import { IconButton } from "../../../ui/icon-button"
import { ResizeEdge } from "../../../ui/resize-edge"
import { focusInFront } from "../../adapters/dom/focus"
import { useWorkspaceFrame } from "../workspace-frame"
import "./right-panel.css"

export const PANEL_ID = "workspace-panel"

export function PanelToggle({
  shown,
  onToggle,
}: {
  shown: boolean
  onToggle: () => void
}) {
  const frame = useWorkspaceFrame()
  return (
    <IconButton
      icon="panelRight"
      label={`${shown ? "Hide" : "Show"} Panel`}
      shortcut={frame.shortcut("togglePanel")}
      aria-expanded={shown}
      aria-controls={PANEL_ID}
      onClick={onToggle}
    />
  )
}

export function PanelMaximize({
  maximized,
  onToggle,
}: {
  maximized: boolean
  onToggle: () => void
}) {
  return (
    <IconButton
      icon={maximized ? "restore" : "maximize"}
      label={maximized ? "Restore Panel" : "Expand Panel"}
      shortcut={maximized ? "Esc" : undefined}
      aria-pressed={maximized}
      aria-controls={PANEL_ID}
      onClick={onToggle}
    />
  )
}

export const WorkspacePanel = memo(function WorkspacePanel({
  open,
  maximized,
  width,
  onDragStart,
  onDragMove,
}: {
  open: boolean
  maximized: boolean
  width: number
  onDragStart: () => void
  onDragMove: (delta: number) => void
}) {
  const panel = useRef<HTMLElement>(null)
  // Whether the caret is in the panel, as its last focus event said.
  const holds = useRef(false)
  // Maximized, the caret under the cover comes into the panel.
  useLayoutEffect(() => {
    if (maximized && !panel.current?.contains(document.activeElement))
      panel.current?.focus()
  }, [maximized])
  // Closed with the caret in it, the caret goes back to the pane in front,
  // rather than falling to the page.
  useLayoutEffect(() => {
    if (open || !holds.current) return
    holds.current = false
    const scope = panel.current?.closest<HTMLElement>("[data-workspace]")
    if (scope) return focusInFront(scope)
  }, [open])
  return (
    <>
      {open && !maximized ? (
        <ResizeEdge
          label="Resize Panel"
          className="workspace-panel-edge"
          value={{ now: width, min: PANEL_MIN_WIDTH, max: PANEL_MAX_WIDTH }}
          onStart={onDragStart}
          onMove={onDragMove}
        />
      ) : null}
      <aside
        ref={panel}
        id={PANEL_ID}
        className="workspace-panel"
        aria-label="Panel"
        tabIndex={-1}
        data-flip="slide"
        data-flip-id="panel"
        data-maximized={maximized || undefined}
        inert={!open || undefined}
        aria-hidden={!open || undefined}
        onFocus={() => {
          holds.current = true
        }}
        onBlur={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null))
            holds.current = false
        }}
      >
        <div className="workspace-panel-body">
          <EmptyState
            variant="compact"
            className="desktop-empty-note"
            title="Nothing open"
          />
        </div>
      </aside>
    </>
  )
})
