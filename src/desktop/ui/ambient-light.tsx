import { useLayoutEffect, useRef } from "react"
import { useElapsed } from "../../clock"

/**
 * The drifting light is painted once into a small texture and scaled up.
 * WebKit re-rasterizes a full-size `filter: blur()` on every frame of a
 * transform animation, and two of those frames are long enough that the
 * Agents overview cannot finish its row-per-frame open inside the 10s
 * wait (#1249). The texture is the circle the stylesheet used to blur,
 * including that blur's spread, so the scaled result matches it.
 */
export const ambientBlurPx = 80
/** Three standard deviations of `ambientBlurPx`, which is where the Gaussian has fallen away. */
export const ambientBlurPadPx = ambientBlurPx * 3

/** How far a texture of `texturePx` must scale to cover the circle plus its blur spread. */
export function ambientScale(boxPx: number, texturePx: number): number {
  return (boxPx + ambientBlurPadPx * 2) / texturePx
}

/**
 * The same colour with its alpha removed, so the gradient fades the light
 * out instead of mixing it toward black. Colours without an alpha stop
 * (a bare `rgb(...)`) fall back to transparent black.
 */
export function transparentStop(color: string): string {
  const match = /^(oklch|oklab|lch|lab|hsl|hsla|rgb|rgba|color)\((.*)\)$/i.exec(
    color.trim(),
  )
  if (!match) return "rgba(0,0,0,0)"
  const slash = match[2].lastIndexOf("/")
  if (slash === -1) return "rgba(0,0,0,0)"
  return `${match[1]}(${match[2].slice(0, slash).trimEnd()} / 0)`
}

/** Paints one light into `canvas` and returns a PNG url, or null when the canvas cannot draw. */
export function paintAmbientLight(
  canvas: HTMLCanvasElement,
  boxPx: number,
  texturePx: number,
  color: string,
): string | null {
  const ctx = canvas.getContext("2d")
  if (!ctx) return null
  const scale = ambientScale(boxPx, texturePx)
  canvas.width = texturePx
  canvas.height = texturePx
  ctx.clearRect(0, 0, texturePx, texturePx)
  ctx.filter = `blur(${ambientBlurPx / scale}px)`
  const center = texturePx / 2
  const radius = boxPx / scale / 2
  ctx.beginPath()
  ctx.arc(center, center, radius, 0, Math.PI * 2)
  const gradient = ctx.createRadialGradient(center, center, 0, center, center, radius)
  gradient.addColorStop(0, color)
  gradient.addColorStop(1, transparentStop(color))
  ctx.fillStyle = gradient
  ctx.fill()
  ctx.filter = "none"
  return canvas.toDataURL("image/png")
}

const lightColorProperties = [
  "--desktop-light-high",
  "--desktop-light-low",
  "--desktop-light-edge",
] as const

/**
 * A theme cross-fade is a transition of one of the light colours on the
 * surface itself. Hover and focus transitions bubble from controls inside
 * that surface and are not one of these.
 */
export function isAmbientLightTransition(
  event: { target: EventTarget | null; propertyName: string },
  surface: EventTarget,
): boolean {
  return (
    event.target === surface &&
    (lightColorProperties as readonly string[]).includes(event.propertyName)
  )
}

/** The 70vmax circle, in CSS pixels, measured the way the stylesheet sizes it. */
function circlePx(): number {
  const probe = document.createElement("div")
  probe.style.cssText =
    "position:absolute;width:70vmax;height:0;visibility:hidden;pointer-events:none"
  document.documentElement.appendChild(probe)
  const box = probe.getBoundingClientRect().width
  probe.remove()
  return box
}

/**
 * The window's ambient light: two drifting washes and the grain over them.
 * Shared by the workspace and Settings so the light has one structure.
 */
export function DesktopAmbient() {
  const host = useRef<HTMLDivElement>(null)
  const high = useRef<HTMLSpanElement>(null)
  const low = useRef<HTMLSpanElement>(null)
  const canvas = useRef<HTMLCanvasElement | null>(null)
  const elapsed = useElapsed()

  useLayoutEffect(() => {
    const root = host.current
    const highLight = high.current
    const lowLight = low.current
    if (!root || !highLight || !lowLight) return
    const surface = root.closest<HTMLElement>("[data-desktop-theme]")
    const board = canvas.current ?? document.createElement("canvas")
    canvas.current = board
    let box = circlePx()
    let painted = ""
    let stop = false
    const paint = () => {
      const texture = parseFloat(
        getComputedStyle(root).getPropertyValue("--ambient-texture"),
      )
      const style = getComputedStyle(root)
      const highColor = style.getPropertyValue("--desktop-light-high").trim()
      const lowColor = style.getPropertyValue("--desktop-light-low").trim()
      if (!box || !texture || !highColor || !lowColor) return
      const key = `${box}|${texture}|${highColor}|${lowColor}`
      if (key === painted) return
      painted = key
      root.style.setProperty("--ambient-scale", String(ambientScale(box, texture)))
      const highUrl = paintAmbientLight(board, box, texture, highColor)
      const lowUrl = paintAmbientLight(board, box, texture, lowColor)
      if (highUrl) highLight.style.backgroundImage = `url("${highUrl}")`
      if (lowUrl) lowLight.style.backgroundImage = `url("${lowUrl}")`
    }
    paint()
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => {
            box = circlePx()
            painted = ""
            paint()
          })
    observer?.observe(document.documentElement)
    // Theme changes interpolate `--desktop-light-*` for 800ms. One frame
    // chain follows that and repaints the texture. The drift itself stays a
    // transform. A second chain is not started while one is already waiting:
    // descendant hover transitions used to each open their own.
    let until = 0
    let pending = false
    let frame = 0
    const follow = () => {
      pending = false
      if (stop) return
      paint()
      if (elapsed() < until) schedule()
    }
    const schedule = () => {
      if (pending || stop) return
      pending = true
      frame = requestAnimationFrame(follow)
    }
    const watch = () => {
      until = elapsed() + 900
      schedule()
    }
    const onTransition = (event: Event) => {
      if (!surface || !(event instanceof TransitionEvent)) return
      if (!isAmbientLightTransition(event, surface)) return
      watch()
    }
    surface?.addEventListener("transitionrun", onTransition)
    const mutations =
      surface === null || typeof MutationObserver === "undefined"
        ? null
        : new MutationObserver(watch)
    if (surface) {
      mutations?.observe(surface, {
        attributes: true,
        attributeFilter: ["data-desktop-theme", "data-tinted-from-picture"],
      })
    }
    return () => {
      stop = true
      if (frame) cancelAnimationFrame(frame)
      observer?.disconnect()
      mutations?.disconnect()
      surface?.removeEventListener("transitionrun", onTransition)
    }
  }, [elapsed])

  return (
    <div className="desktop-ambient" ref={host} aria-hidden="true">
      <span ref={high} className="desktop-ambient-light desktop-ambient-light-high" />
      <span ref={low} className="desktop-ambient-light desktop-ambient-light-low" />
      <span className="desktop-grain" />
    </div>
  )
}
