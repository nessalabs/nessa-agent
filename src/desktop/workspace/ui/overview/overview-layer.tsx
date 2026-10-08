import { useCallback, useRef, useState, type RefObject } from "react"
import { focusInFront } from "../../adapters/dom/focus"
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
 * drawn again.
 */
export function OverviewLayer({ root }: { root: RefObject<HTMLElement | null> }) {
  const dispatch = useWorkspaceDispatch()
  const open = useWorkspaceSelector(selectOverviewOpen)
  const layer = useRef<HTMLDivElement>(null)
  const split = useAtLeastWide(layer, splitWidth)
  // The list's width beside the peek, as the person dragged it; null is the
  // even split the stylesheet draws. Held here, where the layer outlives each
  // opening, so the overview opens as it was left for as long as the window
  // is; it is a view's arrangement, not the workspace's state.
  const [listWidth, setListWidth] = useState<number | null>(null)
  const stop = useRef<() => void>(() => {})
  const leave = useCallback(() => {
    dispatch(showContent({ content: "panes" }))
    stop.current()
    stop.current = focusInFront(root.current ?? document)
  }, [dispatch, root])
  return (
    <div
      className="workspace-overview-layer"
      ref={layer}
      data-open={open || undefined}
      data-flip="slide"
      data-flip-id="overview"
    >
      {open ? (
        <AgentsOverview
          split={split}
          listWidth={listWidth}
          onListWidth={setListWidth}
          onLeave={leave}
        />
      ) : null}
    </div>
  )
}
