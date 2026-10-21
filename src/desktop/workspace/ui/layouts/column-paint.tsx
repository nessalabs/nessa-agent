import { useEffect, useLayoutEffect, useRef, type RefObject } from "react"
import { drawn } from "../../../model/side-column"
import { useFitOnResize } from "../../adapters/dom/window-width"
import { fitToWindow } from "../../adapters/store/commands"
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../../adapters/store/hooks"
import {
  listMeasureEpoch,
  listMeasureHeld,
  listPaintFrame,
  spareFoldFrame,
  usePaintedWithPanes,
} from "../../adapters/store/paint-with-panes"
import { selectChrome, selectColumnCount } from "../../adapters/store/selectors"

/**
 * The fold's attribute, without rendering the window. It lands while the
 * spare hold keeps the sidebar in the row. The window renders the same
 * fold with the list, on the frame after the boxes.
 */
export function FoldMark({ root }: { root: RefObject<HTMLElement | null> }) {
  const chrome = usePaintedWithPanes(selectChrome, spareFoldFrame)
  const open = drawn(chrome.sidebar)
  useLayoutEffect(() => {
    const workspace = root.current
    const frame = workspace?.closest<HTMLElement>(".workspace-window")
    const value = open ? "open" : "closed"
    workspace?.setAttribute("data-sidebar", value)
    frame?.setAttribute("data-sidebar", value)
  }, [open, root])
  return null
}

/**
 * Holds the session list's measure through the fold and the frame that
 * shows the scene text again. The list takes its closed measure on the
 * frame after that (`listPaintFrame`), so that commit does not restyle
 * the rows on the scene's frame. A column change that puts the previous
 * count back before that frame keeps the hold until the new list frame:
 * the boxes on screen are still the ones just painted. A shape change
 * that returns before the next frame cancels the hold that frame would
 * have written.
 */
export function ListMeasureHold({ root }: { root: RefObject<HTMLElement | null> }) {
  const store = useWorkspaceStore()
  // The epoch changes on the list frame even when the count returns to the
  // one the list already measured, which is what releases the hold.
  const epoch = usePaintedWithPanes(() => listMeasureEpoch(store), listPaintFrame)
  const held = useRef(false)
  useEffect(() => {
    let frame = 0
    const apply = (on: boolean) => {
      held.current = on
      root.current?.toggleAttribute("data-list-held", on)
    }
    apply(listMeasureHeld(store))
    const unsubscribe = store.subscribe(() => {
      const on = listMeasureHeld(store)
      // The queued frame has not applied yet, so `held` still matches the
      // screen. Cancel it before that comparison: a reversal in the same
      // turn would otherwise leave the frame to write the hold after the
      // gate has stopped.
      cancelAnimationFrame(frame)
      frame = 0
      if (on === held.current) return
      // The click does not write the attribute. The next frame does, which
      // is before the fold paints.
      frame = requestAnimationFrame(() => apply(on))
    })
    return () => {
      cancelAnimationFrame(frame)
      unsubscribe()
    }
  }, [store, root])
  useEffect(() => {
    const on = listMeasureHeld(store)
    held.current = on
    root.current?.toggleAttribute("data-list-held", on)
  }, [epoch, store, root])
  return null
}

/**
 * Fits the side columns from the column count, without rendering the
 * window. The count is a layout selector, so the click that changes it
 * does not render this: the count arrives the next frame, and the fit
 * runs the frame after that.
 */
export function ColumnFit({
  windowWidth,
  sidebarWidth,
  listWidth,
}: {
  windowWidth: number
  sidebarWidth: number
  listWidth: number
}) {
  const dispatch = useWorkspaceDispatch()
  const columns = useWorkspaceSelector(selectColumnCount)
  useFitOnResize(windowWidth, columns, (width) =>
    dispatch(
      fitToWindow({ windowWidth: width, sidebarWidth, sessionListWidth: listWidth }),
    ),
  )
  return null
}
