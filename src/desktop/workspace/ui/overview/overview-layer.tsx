import { useCallback, useLayoutEffect, useRef, type RefObject } from "react"
import { isMac } from "../../../adapters/platform"
import { matchesChord } from "../../../model/keyboard"
import { focusInFront } from "../../adapters/dom/focus"
import { useAtLeastWide } from "../../adapters/dom/width"
import { showContent, showOverviewGroup } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectOverviewGroup, selectOverviewOpen } from "../../adapters/store/selectors"
import { AgentsOverview, splitWidth } from "./overview"
import { overviewKeys } from "./overview-keys"

/**
 * Where the Agents overview is drawn: a layer over the session list and the
 * panes, which stay laid out beneath it — hidden and out of reach, at the
 * size and scroll they had — so the room a pane command measures is the
 * panes' own whether the overview is open or not, and coming back lays
 * nothing out again. The layer is on the page with the workspace, open or
 * not, so its width — whether the peek fits beside the list — is known
 * before the overview opens, and its first frame is laid out once.
 *
 * Escape is installed on the commit that opens, before paint, so it leaves
 * even though the list is not drawn yet (`overview.test.tsx`).
 *
 * Leaving it — Escape, Open, or going anywhere else — gives the keyboard
 * back to the focused pane's composer, a frame later, once the panes are
 * drawn again.
 */
export function OverviewLayer({ root }: { root: RefObject<HTMLElement | null> }) {
  const dispatch = useWorkspaceDispatch()
  const open = useWorkspaceSelector(selectOverviewOpen)
  const group = useWorkspaceSelector(selectOverviewGroup)
  const layer = useRef<HTMLDivElement>(null)
  const split = useAtLeastWide(layer, splitWidth)
  const stop = useRef<() => void>(() => {})
  const leave = useCallback(() => {
    dispatch(showContent({ content: "panes" }))
    stop.current()
    stop.current = focusInFront(root.current ?? document)
  }, [dispatch, root])
  const leaveNow = useRef(leave)
  leaveNow.current = leave
  const groupNow = useRef(group)
  groupNow.current = group
  // Escape leaves whenever the overview is open — wherever the keyboard is,
  // even before it has landed on a row — but for Escape in a menu or a
  // dialog over it, which is theirs, and under Settings, whose keys are its
  // own; and, with one group shown alone, the first Escape shows every group
  // again and the next leaves.
  useLayoutEffect(() => {
    if (!open) return
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return
      const binding = overviewKeys.find(
        (candidate) =>
          candidate.command === "leave" && matchesChord(event, candidate.chord, isMac),
      )
      if (!binding) return
      if (layer.current?.closest("[inert]")) return
      if (
        event.target instanceof Element &&
        event.target.closest('[role="dialog"], [role="menu"], [role="listbox"]')
      )
        return
      event.preventDefault()
      if (groupNow.current !== null) dispatch(showOverviewGroup({ group: null }))
      else leaveNow.current()
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [open, dispatch])
  return (
    <div className="workspace-overview-layer" ref={layer} data-open={open || undefined}>
      {open ? <AgentsOverview split={split} onLeave={leave} /> : null}
    </div>
  )
}
