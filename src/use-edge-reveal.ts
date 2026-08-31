import * as React from "react"

/** How far from an edge, in px, the pointer starts waking the border. */
const REACH = 64
/** Inside this distance the reveal holds full strength. */
const CONTACT = 6
/**
 * The ceiling on the glow. The rim colours are saturated enough that a full
 * pass reads as a highlight rather than a hint; the edge should suggest it can
 * be grabbed, not announce it.
 */
const MAX_STRENGTH = 0.55

/**
 * Drives the panel border's proximity reveal.
 *
 * The pointer's position is written to the glow element as CSS custom
 * properties and an opacity, from an animation frame — never through state, so
 * moving the mouse cannot re-render the conversation tree. The glow itself is
 * a radial gradient masked down to the border ring (see `.nessa-edge-reveal`),
 * which is what makes only the stretch of border nearest the pointer light up
 * instead of the whole outline.
 *
 * Strength follows the distance to the *nearest* edge, so the border answers
 * wherever the pointer approaches it — not only at the resize handle.
 */
export function useEdgeReveal() {
  const glowRef = React.useRef<HTMLDivElement>(null)
  const panel = React.useRef<HTMLElement | null>(null)
  const point = React.useRef({ x: 0, y: 0 })
  const frame = React.useRef(0)
  // A native resize drag is run by macOS, not by the page: the webview sees a
  // pointerleave as the drag takes over and few or no events until the mouse
  // is released. The glow therefore pins itself for the length of the drag
  // rather than trying to track a distance that the moving window edge keeps
  // redefining underneath it.
  const resizing = React.useRef(false)

  React.useEffect(() => () => cancelAnimationFrame(frame.current), [])

  const paint = React.useCallback(() => {
    frame.current = 0
    const glow = glowRef.current
    const node = panel.current
    if (!glow || !node) return

    const box = node.getBoundingClientRect()
    const x = point.current.x - box.left
    const y = point.current.y - box.top

    // The gradient is centred on the pointer; the mask keeps only the part of
    // it that falls on the border, so corners are handled by the geometry
    // rather than by four separate cases.
    glow.style.setProperty("--edge-x", `${x}px`)
    glow.style.setProperty("--edge-y", `${y}px`)

    if (resizing.current) {
      // Mid-drag the panel's edge is moving with the pointer, so a computed
      // distance would flicker; the drag itself is the reason to stay lit.
      glow.style.opacity = String(MAX_STRENGTH)
      return
    }

    const distance = Math.min(x, y, box.width - x, box.height - y)
    const near = Math.min(Math.max((REACH - distance) / (REACH - CONTACT), 0), 1)
    // Squared, so the border stirs faintly at the fringe of the reach and
    // commits as the pointer closes in rather than ramping linearly.
    glow.style.opacity = String(near * near * MAX_STRENGTH)
  }, [])

  const onPointerMove = React.useCallback(
    (event: React.PointerEvent<HTMLElement>) => {
      // Only a move with no button held ends a drag. Clearing the flag on any
      // move let a single stray event between pointerdown and the native
      // drag's takeover cancel it, and the pointerleave that followed then
      // put the glow out for the rest of the resize.
      if (event.buttons === 0) resizing.current = false
      panel.current = event.currentTarget
      point.current = { x: event.clientX, y: event.clientY }
      if (frame.current) return
      frame.current = requestAnimationFrame(paint)
    },
    [paint],
  )

  const onPointerLeave = React.useCallback(() => {
    if (resizing.current) return
    cancelAnimationFrame(frame.current)
    frame.current = 0
    if (glowRef.current) glowRef.current.style.opacity = "0"
  }, [])

  const onResizeStart = React.useCallback(() => {
    resizing.current = true
    if (glowRef.current) glowRef.current.style.opacity = String(MAX_STRENGTH)

    // The release may land outside the webview, where no React handler will
    // see it — without this the glow would stay pinned until the pointer
    // happened to move over the panel again.
    const release = () => {
      resizing.current = false
      window.removeEventListener("mouseup", release, true)
      if (!frame.current) frame.current = requestAnimationFrame(paint)
    }
    window.addEventListener("mouseup", release, true)
  }, [paint])

  return { glowRef, onPointerMove, onPointerLeave, onResizeStart }
}
