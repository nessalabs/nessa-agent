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
 * the rows on the scene's frame.
 */
export function ListMeasureHold({ root }: { root: RefObject<HTMLElement | null> }) {
  const store = useWorkspaceStore()
  const painted = usePaintedWithPanes(selectColumnCount, listPaintFrame)
  const paintedRef = useRef(painted)
  paintedRef.current = painted
  useEffect(() => {
    let frame = 0
    let held = false
    const apply = (on: boolean) => {
      root.current?.toggleAttribute("data-list-held", on)
    }
    apply(selectColumnCount(store.getState()) !== paintedRef.current)
    const unsubscribe = store.subscribe(() => {
      const on = selectColumnCount(store.getState()) !== paintedRef.current
      if (on === held) return
      held = on
      cancelAnimationFrame(frame)
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
    root.current?.toggleAttribute(
      "data-list-held",
      selectColumnCount(store.getState()) !== painted,
    )
  }, [painted, store, root])
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
