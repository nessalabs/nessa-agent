import { useCallback, useEffect, useRef, type RefObject } from "react"
import { useAgentsOverviewPreference } from "../../../adapters/window-preferences"
import { focusComposer } from "../../adapters/dom/focus"
import { useAtLeastWide } from "../../adapters/dom/width"
import { showContent } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectOverviewOpen } from "../../adapters/store/selectors"
import { AgentsOverview, splitWidth } from "./overview"

/**
 * Where the Agents overview is drawn: a layer over the session list and the
 * panes, which stay laid out beneath it — hidden and out of reach, at the
 * size and scroll they had — so the room a pane command measures is the
 * panes' own whether the overview is open or not, and coming back lays
 * nothing out again. The layer is on the page with the workspace, open or
 * not, so its width — whether the peek fits beside the list — is known
 * before the overview opens, and its first frame is laid out once.
 *
 * Leaving it — Escape, Open, or going anywhere else — gives the keyboard
 * back to the focused pane's composer, a frame later, once the panes are
 * drawn again. Turning the preview off (Settings › General › Experimental,
 * in this window or another beside it) leaves it the same way: the one place
 * that change closes it.
 */
export function OverviewLayer({ root }: { root: RefObject<HTMLElement | null> }) {
  const dispatch = useWorkspaceDispatch()
  const open = useWorkspaceSelector(selectOverviewOpen)
  const layer = useRef<HTMLDivElement>(null)
  const split = useAtLeastWide(layer, splitWidth)
  const stop = useRef<() => void>(() => {})
  const leave = useCallback(() => {
    dispatch(showContent({ content: "panes" }))
    stop.current()
    stop.current = focusComposer(root.current ?? document)
  }, [dispatch, root])
  // The preview turned off, from wherever: an open overview gives the region back.
  const [offered] = useAgentsOverviewPreference()
  const offeredBefore = useRef(offered)
  const openNow = useRef(open)
  openNow.current = open
  useEffect(() => {
    const turnedOff = offeredBefore.current === "on" && offered !== "on"
    offeredBefore.current = offered
    if (turnedOff && openNow.current) leave()
  }, [offered, leave])
  return (
    <div className="workspace-overview-layer" ref={layer} data-open={open || undefined}>
      {open ? <AgentsOverview split={split} onLeave={leave} /> : null}
    </div>
  )
}
