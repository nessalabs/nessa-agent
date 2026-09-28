import { useRef } from "react"
import type { EdgePeekControls } from "../adapters/use-edge-peek"
import { draggedEdge } from "../model/side-column"

/**
 * The strip along the window's left edge where a folded sidebar waits:
 * resting the pointer on it reveals the sidebar (`useEdgePeek`), and
 * dragging out from it opens the sidebar for good (`onDragOut`, once the
 * drag has come `collapseSnap` out) — the way a sidebar dragged closed comes
 * back. Drawn only while the sidebar is folded; the same strip in every
 * surface that has one.
 */
export function EdgePeekStrip({
  peek,
  onDragOut,
}: {
  peek: EdgePeekControls
  onDragOut?: () => void
}) {
  const from = useRef<number | null>(null)
  return (
    <div
      className="desktop-peek-edge"
      aria-hidden="true"
      onPointerEnter={peek.enter}
      onPointerLeave={peek.leave}
      onPointerDown={(event) => {
        if (!onDragOut || event.button !== 0) return
        event.preventDefault()
        from.current = event.clientX
        event.currentTarget.setPointerCapture(event.pointerId)
      }}
      onPointerMove={(event) => {
        if (from.current === null) return
        const { open } = draggedEdge(null, event.clientX - from.current, {
          min: 0,
          max: 0,
        })
        if (!open) return
        from.current = null
        onDragOut?.()
      }}
      onPointerUp={() => (from.current = null)}
      onPointerCancel={() => (from.current = null)}
    />
  )
}
