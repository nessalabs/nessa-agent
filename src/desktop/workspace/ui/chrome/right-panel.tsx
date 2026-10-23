/**
 * The right panel in the workspace: Classic's panel (`ui/right-panel.tsx`
 * holds the parts both share), as a column beside the panes. Its edge
 * resizes it, and dragged far enough right folds it. Maximized, it covers the
 * workspace — the sidebar, the list and the panes are inert under it — until
 * Escape or Restore.
 */
import { memo, useLayoutEffect, useRef, type RefObject } from "react"
import { PANEL_MAX_WIDTH, PANEL_MIN_WIDTH } from "../../../model/right-panel"
import { ResizeEdge } from "../../../ui/resize-edge"
import { PanelEmpty } from "../../../ui/right-panel"
import "./right-panel.css"

export const PANEL_ID = "workspace-panel"

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
          <PanelEmpty />
        </div>
      </aside>
    </>
  )
})
