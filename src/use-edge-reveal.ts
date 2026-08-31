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

    // Outside the panel there is no border to reveal. Without this the
    // distance goes negative, clamps to full strength, and a drag released
    // beyond the window's edge leaves the glow lit with no further
    // pointerleave coming to put it out.
    if (x < 0 || y < 0 || x > box.width || y > box.height) {
      glow.style.opacity = "0"
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
      // Nothing here ends a drag. A native drag is run by the window server,
      // which synthesises moves that do not reliably carry button state, so
      // inferring the end from `buttons` cut the pin short mid-resize — the
      // glow then followed plain proximity, which still lights near where the
      // drag began and dies as soon as the pointer travels along the edge.
      // Only a real release ends it; see `onResizeStart`.
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

    // The pin is released only by a real end-of-drag, never inferred from
    // pointer movement. The release can land outside the webview and the
    // window server does not always hand the page a matching event, so every
    // signal that means "the drag is over" is listened for and the first one
    // to arrive wins. Losing focus counts: a drag cannot still be running.
    const endings = ["mouseup", "pointerup", "pointercancel", "blur"] as const
    const release = () => {
      resizing.current = false
      for (const ending of endings) {
        window.removeEventListener(ending, release, true)
      }
      if (!frame.current) frame.current = requestAnimationFrame(paint)
    }
    for (const ending of endings) {
      window.addEventListener(ending, release, true)
    }
  }, [paint])

  return { glowRef, onPointerMove, onPointerLeave, onResizeStart }
}
