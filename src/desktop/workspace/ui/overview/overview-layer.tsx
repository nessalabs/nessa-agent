import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type RefObject,
} from "react"
import { flushSync } from "react-dom"
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
 * panes, which stay laid out beneath it — out of reach, at the size and
 * scroll they had — so the room a pane command measures is the
 * panes' own whether the overview is open or not, and coming back lays
 * nothing out again. The layer is on the page with the workspace, open or
 * not, so its width — whether the peek fits beside the list — is known
 * before the overview opens, and its first frame is laid out once.
 *
 * Escape is installed on the commit that opens, before paint, so it leaves
 * even though the list is not drawn yet (`overview.test.tsx`).
 *
 * Opening is four paints (`overview.test.tsx`). The commit sets the content
 * and `data-open` only. The next frame marks the glass
 * (`data-overview-glass`), which quiets the sidebar's rows. The frame after covers the list and the panes
 * (`data-overview-covered` on the workspace, `data-covered` on this layer).
 * The frame after that mounts the overview. Leaving lifts the cover and drops
 * the overview in that commit (`overview.test.tsx`).
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
  // Cover, then the overview, each on its own frame after the open commit.
  // Both follow `open`, so a leave renders neither (`overview.test.tsx`).
  const [coverReady, setCoverReady] = useState(false)
  const [mountReady, setMountReady] = useState(false)
  const covered = open && coverReady
  // Unmounted in the leave's own commit. Keeping it and hiding it would
  // restyle every row on that key (`overview.test.tsx`).
  const mount = open && mountReady
  useEffect(() => {
    if (!open) {
      // After the leave has painted, so that paint still has the rows quiet.
      // The first paint has nothing to drop (`overview.test.tsx`).
      root.current?.removeAttribute("data-overview-glass")
      if (!coverReady && !mountReady) return
      // After the leave has painted. Focusing waits a frame, so it does not
      // measure the panes in that same turn (`overview.test.tsx`).
      setCoverReady(false)
      setMountReady(false)
      const frame = requestAnimationFrame(() => {
        stop.current = focusInFront(root.current ?? document)
      })
      return () => cancelAnimationFrame(frame)
    }
    let coverFrame = 0
    let mountFrame = 0
    // The open paint is only `data-open`. This frame quiets the sidebar's
    // rows, and the cover waits one more, so the key itself does not restyle
    // the sidebar (`overview.test.tsx`).
    const glassFrame = requestAnimationFrame(() => {
      root.current?.setAttribute("data-overview-glass", "")
      coverFrame = requestAnimationFrame(() => {
        // Committed before the callback returns, so this frame paints the
        // cover and the next frame paints the mount (`overview.test.tsx`).
        flushSync(() => setCoverReady(true))
        mountFrame = requestAnimationFrame(() => {
          flushSync(() => setMountReady(true))
        })
      })
    })
    return () => {
      cancelAnimationFrame(glassFrame)
      cancelAnimationFrame(coverFrame)
      cancelAnimationFrame(mountFrame)
    }
  }, [open])
  // What the cover stands over. `inert` takes it out of reach without
  // `visibility`, which would restyle every descendant as the cover comes
  // and goes. Lifting it waits a frame: doing it in the leave's commit lays
  // the panes out on that key (`overview.test.tsx`).
  const stilled = useRef<HTMLElement[]>([])
  useLayoutEffect(() => {
    const node = root.current
    if (!node) return
    if (covered) {
      node.setAttribute("data-overview-covered", "")
      stilled.current = [
        ".workspace-list",
        ".workspace-list-edge",
        ".workspace-chat",
      ].flatMap((selector) => {
        const element = node.querySelector<HTMLElement>(selector)
        if (!element) return []
        element.setAttribute("inert", "")
        return [element]
      })
      return
    }
    node.removeAttribute("data-overview-covered")
    const frame = requestAnimationFrame(() => {
      for (const element of stilled.current) element.removeAttribute("inert")
      stilled.current = []
    })
    return () => cancelAnimationFrame(frame)
  }, [covered, root])
  const stop = useRef<() => void>(() => {})
  const leave = useCallback(() => {
    dispatch(showContent({ content: "panes" }))
    stop.current()
  }, [dispatch])
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
    <div
      className="workspace-overview-layer"
      ref={layer}
      data-open={open || undefined}
      data-covered={covered || undefined}
    >
      {mount ? <AgentsOverview split={split} onLeave={leave} /> : null}
    </div>
  )
}
