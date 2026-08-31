import * as React from "react"

import { onLiveResize, onWindowResize } from "./host-window"

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

type Edge = "left" | "right" | "top" | "bottom"

/** Which edge of the box a point is nearest, from inside or outside it. */
function nearestEdge(x: number, y: number, box: DOMRect): Edge {
  const px = Math.min(Math.max(x, 0), box.width)
  const py = Math.min(Math.max(y, 0), box.height)
  const nearest = Math.min(px, box.width - px, py, box.height - py)
  if (nearest === px) return "left"
  if (nearest === box.width - px) return "right"
  if (nearest === py) return "top"
  return "bottom"
}

/**
 * Walks a point onto one edge of the box, keeping the coordinate that runs
 * along that edge where it was.
 *
 * Mid-drag the pointer's position is a relic: the page is given no pointer
 * events while macOS runs the gesture, so the gradient stays centred wherever
 * the pointer was when the drag began while the panel changes shape around it.
 * The glow is masked to the border ring and the gradient fades out well inside
 * its own radius, so a centre the ring has moved away from lights nothing at
 * all — the whole border goes dark mid-resize with the opacity still at full.
 *
 * Clamping into the box instead would answer only the half of the problem
 * where the panel shrinks; dragging an edge outward leaves the point stranded
 * in the interior and the ring recedes out of reach with the opposite sign.
 * Projecting covers both, and the edge is the one the point was nearest when
 * the drag began — which is the one being dragged, since that is where the
 * pointer had to be to take hold of it. Re-choosing the nearest edge on every
 * frame instead would hand the glow to a border the reader is not dragging as
 * soon as the panel grew wider than the pointer's distance along it.
 */
function ontoEdge(edge: Edge, x: number, y: number, box: DOMRect) {
  const px = Math.min(Math.max(x, 0), box.width)
  const py = Math.min(Math.max(y, 0), box.height)
  switch (edge) {
    case "left":
      return { x: 0, y: py }
    case "right":
      return { x: box.width, y: py }
    case "top":
      return { x: px, y: 0 }
    case "bottom":
      return { x: px, y: box.height }
  }
}

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
  const panelRef = React.useRef<HTMLDivElement>(null)
  const point = React.useRef({ x: 0, y: 0 })
  const frame = React.useRef(0)
  // A resize drag belongs to macOS, not to the page: the webview stops seeing
  // pointer events the moment the drag starts and does not see them again
  // until it ends. The glow therefore pins itself for the length of the drag
  // rather than trying to track a distance that the moving window edge keeps
  // redefining underneath it.
  const resizing = React.useRef(false)
  const grabbed = React.useRef<Edge>("left")

  React.useEffect(() => () => cancelAnimationFrame(frame.current), [])

  const paint = React.useCallback(() => {
    frame.current = 0
    const glow = glowRef.current
    const node = panelRef.current
    if (!glow || !node) return

    const box = node.getBoundingClientRect()
    const at = { x: point.current.x - box.left, y: point.current.y - box.top }
    // Pinned, the gradient rides the edge being dragged instead of sitting
    // where the pointer last was, which the panel is moving away from.
    const centre = resizing.current
      ? ontoEdge(grabbed.current, at.x, at.y, box)
      : at

    // The gradient is centred on the pointer; the mask keeps only the part of
    // it that falls on the border, so corners are handled by the geometry
    // rather than by four separate cases.
    glow.style.setProperty("--edge-x", `${centre.x}px`)
    glow.style.setProperty("--edge-y", `${centre.y}px`)

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
    if (at.x < 0 || at.y < 0 || at.x > box.width || at.y > box.height) {
      glow.style.opacity = "0"
      return
    }

    const distance = Math.min(at.x, at.y, box.width - at.x, box.height - at.y)
    const near = Math.min(Math.max((REACH - distance) / (REACH - CONTACT), 0), 1)
    // Squared, so the border stirs faintly at the fringe of the reach and
    // commits as the pointer closes in rather than ramping linearly.
    glow.style.opacity = String(near * near * MAX_STRENGTH)
  }, [])

  /**
   * The pin is held for exactly as long as the window's frame is held.
   *
   * The panel draws its own left-edge handle, but on a borderless resizable
   * NSWindow macOS claims the frame first: it swallows the mousedown and runs
   * the resize itself, so a pin armed from the handle's own `pointerdown`
   * never arms at all. What stays on screen for the drag is then whatever
   * opacity `paint` last wrote before the page went blind — full strength if
   * the pointer settled on the edge, nothing if it clipped outside the panel
   * on the way in, which is the whole of why the same handle appeared to work
   * or fail depending on where it was grabbed.
   *
   * The size stream is subscribed to separately and only to repaint: it says
   * the panel has changed shape, which is what moves the gradient along the
   * edge, but it is silent through the stretches of a drag that change no size
   * and so can say nothing about when one ends.
   */
  React.useEffect(() => {
    const repaint = () => {
      if (!frame.current) frame.current = requestAnimationFrame(paint)
    }
    const live = onLiveResize((active) => {
      if (active && !resizing.current) {
        // Read the grabbed edge from the shape the panel still has, before the
        // drag starts changing it.
        const box = panelRef.current?.getBoundingClientRect()
        if (box) {
          grabbed.current = nearestEdge(
            point.current.x - box.left,
            point.current.y - box.top,
            box,
          )
        }
      }
      resizing.current = active
      repaint()
    })
    const sized = onWindowResize(repaint)
    return () => {
      void live.then((unlisten) => unlisten())
      void sized.then((unlisten) => unlisten())
    }
  }, [paint])

  const onPointerMove = React.useCallback(
    (event: React.PointerEvent<HTMLElement>) => {
      point.current = { x: event.clientX, y: event.clientY }
      if (frame.current) return
      frame.current = requestAnimationFrame(paint)
    },
    [paint],
  )

  // Taking hold of the frame means going to the very edge, and macOS's grab
  // zone reaches past it, so the pointer often crosses out of the panel just
  // before a drag. Putting the glow out here is still right: the drag lights
  // it again the moment AppKit says one has started.
  // Hiding the panel fires no pointerleave — the window simply goes away — so
  // without this the last opacity survives until the pointer next crosses the
  // panel, and the border comes back lit on a summon from across the screen.
  // Losing focus is the one signal that always accompanies being hidden.
  React.useEffect(() => {
    const clear = () => {
      // A native resize blurs the window too, and the pin has to outlive that.
      if (resizing.current) return
      cancelAnimationFrame(frame.current)
      frame.current = 0
      if (glowRef.current) glowRef.current.style.opacity = "0"
    }
    window.addEventListener("blur", clear)
    return () => window.removeEventListener("blur", clear)
  }, [])

  const onPointerLeave = React.useCallback(() => {
    if (resizing.current) return
    cancelAnimationFrame(frame.current)
    frame.current = 0
    if (glowRef.current) glowRef.current.style.opacity = "0"
  }, [])

  /**
   * Pins the glow for a resize the page itself started. On Linux the left-edge
   * handle is the drag, so pointerdown/up here is a complete signal. On macOS
   * the system usually claims the frame first and these never fire; AppKit's
   * live-resize notifications cover that path instead.
   */
  const holdResize = React.useCallback(() => {
    const box = panelRef.current?.getBoundingClientRect()
    if (box) {
      grabbed.current = nearestEdge(
        point.current.x - box.left,
        point.current.y - box.top,
        box,
      )
    }
    resizing.current = true
    if (!frame.current) frame.current = requestAnimationFrame(paint)
  }, [paint])

  const releaseResize = React.useCallback(() => {
    resizing.current = false
    if (!frame.current) frame.current = requestAnimationFrame(paint)
  }, [paint])

  return {
    glowRef,
    panelRef,
    onPointerMove,
    onPointerLeave,
    holdResize,
    releaseResize,
  }
}
