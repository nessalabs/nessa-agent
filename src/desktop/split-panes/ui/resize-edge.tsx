import type { CSSProperties, PointerEvent as ReactPointerEvent } from "react"
import { usePointerResize } from "../adapters/dom/pointer-resize"
import { tooltip } from "../../ui/tooltip"

/** How far an arrow key moves an edge. */
const keyStep = 16

/**
 * A boundary that resizes what is on either side of it: dragged, or moved by
 * the arrow keys when focused. A glow runs along it near the pointer, in the
 * theme's edge light.
 */
export function ResizeEdge({
  label,
  axis = "x",
  className,
  style,
  onStart,
  onMove,
  onReset,
  value,
}: {
  label: string
  /**
   * Where the edge stands, for assistive technology: the side before it, in
   * the unit the edge is moved in — a column's pixels, or the share of the
   * two panes it sits between, in percent — within the bounds it is held to.
   */
  value: { readonly now: number; readonly min: number; readonly max: number }
  /** The direction the edge moves in: "x" for a vertical edge, "y" for a horizontal one. */
  axis?: "x" | "y"
  className?: string
  style?: CSSProperties
  /** Called as a drag or key press begins, to take the sizes it starts from. */
  onStart: () => void
  /** The edge's travel since `onStart`, in pixels. */
  onMove: (delta: number) => void
  /** A double-click: back to even. */
  onReset?: () => void
}) {
  const drag = usePointerResize(axis, { onStart, onMove })
  const glow = (event: ReactPointerEvent<HTMLDivElement>) => {
    const box = event.currentTarget.getBoundingClientRect()
    const along = axis === "x" ? event.clientY - box.top : event.clientX - box.left
    event.currentTarget.style.setProperty("--workspace-glow", `${along}px`)
  }
  const [back, forward] =
    axis === "x" ? ["ArrowLeft", "ArrowRight"] : ["ArrowUp", "ArrowDown"]
  return (
    <div
      role="separator"
      aria-orientation={axis === "x" ? "vertical" : "horizontal"}
      aria-label={label}
      aria-valuenow={Math.round(value.now)}
      aria-valuemin={Math.round(value.min)}
      aria-valuemax={Math.round(value.max)}
      tabIndex={0}
      className={className ? `workspace-edge ${className}` : "workspace-edge"}
      data-axis={axis}
      style={style}
      {...drag}
      onPointerMove={(event) => {
        glow(event)
        drag.onPointerMove(event)
      }}
      onPointerEnter={glow}
      onDoubleClick={onReset}
      {...(onReset ? tooltip(`${label} · double-click to even out`) : {})}
      onKeyDown={(event) => {
        const step = event.key === back ? -keyStep : event.key === forward ? keyStep : 0
        if (!step) return
        event.preventDefault()
        onStart()
        onMove(step)
      }}
    />
  )
}
