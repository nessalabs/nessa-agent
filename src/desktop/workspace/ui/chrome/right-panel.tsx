/**
 * The right panel: a column beside the panes, in both layouts, with its
 * toggle and maximize button at the titlebar's end (⌥⌘B shows and hides it).
 * Its edge resizes it, and dragged far enough right folds it. Maximized, it
 * covers the workspace — the sidebar, the list and the panes are inert under
 * it — until Escape or Restore. Until something opens in it, it says so.
 */
import { memo, useLayoutEffect, useRef, type RefObject } from "react"
import { EmptyState } from "@nessa-ui/react/empty-state"
import { PANEL_MAX_WIDTH, PANEL_MIN_WIDTH } from "../../../model/right-panel"
import { IconButton } from "../../../ui/icon-button"
import { ResizeEdge } from "../../../ui/resize-edge"
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
  root,
  open,
  maximized,
  width,
  onDragStart,
  onDragMove,
}: {
  /** The workspace, whose other regions go inert under the maximized panel. */
  root: RefObject<HTMLDivElement | null>
  open: boolean
  maximized: boolean
  width: number
  onDragStart: () => void
  onDragMove: (delta: number) => void
}) {
  const panel = useRef<HTMLElement>(null)
  useLayoutEffect(() => {
    const scope = root.current
    if (!maximized || !scope) return
    const covered = [...scope.children].filter(
      (child) => child !== panel.current && !child.hasAttribute("inert"),
    )
    for (const child of covered) child.setAttribute("inert", "")
    // The caret was under the cover: it comes into the panel.
    if (!panel.current?.contains(document.activeElement)) panel.current?.focus()
    return () => {
      // A column that folded meanwhile is hidden by its own markup: it stays inert.
      for (const child of covered)
        if (child.getAttribute("aria-hidden") !== "true") child.removeAttribute("inert")
    }
  }, [maximized, root])
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
