/**
 * Dragging a boundary: reports the pointer's travel along one axis since the
 * press, with the pointer captured so the drag survives leaving the edge.
 * Moves are coalesced to one per frame. While a drag runs, the document is
 * marked `data-workspace-resizing` with the axis, so the cursor holds
 * everywhere and nothing selects text — and the mark goes however the drag
 * ends: released, cancelled, capture lost, or the edge itself unmounted.
 */
import { useEffect, useRef, type PointerEvent as ReactPointerEvent } from "react"

export function usePointerResize(
  axis: "x" | "y",
  {
    onStart,
    onMove,
  }: {
    onStart: () => void
    onMove: (delta: number) => void
  },
) {
  const start = useRef<number | null>(null)
  const frame = useRef(0)
  const at = (event: ReactPointerEvent<HTMLElement>) =>
    axis === "x" ? event.clientX : event.clientY
  const latest = useRef({ onMove })
  latest.current = { onMove }

  const end = useRef(() => {
    if (start.current === null) return
    start.current = null
    cancelAnimationFrame(frame.current)
    delete document.documentElement.dataset.workspaceResizing
  })
  useEffect(() => {
    const finish = end.current
    return finish
  }, [])

  return {
    onPointerDown(event: ReactPointerEvent<HTMLElement>) {
      // The primary button only: a right-click is a menu, not a drag.
      if (event.button !== 0) return
      event.preventDefault()
      onStart()
      event.currentTarget.setPointerCapture(event.pointerId)
      start.current = at(event)
      document.documentElement.dataset.workspaceResizing = axis
    },
    onPointerMove(event: ReactPointerEvent<HTMLElement>) {
      if (start.current === null) return
      const delta = at(event) - start.current
      cancelAnimationFrame(frame.current)
      frame.current = requestAnimationFrame(() => latest.current.onMove(delta))
    },
    onPointerUp(event: ReactPointerEvent<HTMLElement>) {
      if (start.current === null) return
      const delta = at(event) - start.current
      cancelAnimationFrame(frame.current)
      latest.current.onMove(delta)
      if (event.currentTarget.hasPointerCapture(event.pointerId))
        event.currentTarget.releasePointerCapture(event.pointerId)
      end.current()
    },
    onPointerCancel: () => end.current(),
    onLostPointerCapture: () => end.current(),
  }
}
