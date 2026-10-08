import { useEffect, useLayoutEffect, useRef } from "react"
import {
  clampHeaderFraming,
  defaultHeaderFraming,
  headerZoomRange,
  panHeaderFraming,
  placeHeaderImage,
  type HeaderFraming,
} from "../model/header-image"
import { IconButton } from "./icon-button"

/** Pixels an arrow key moves the picture, and how much a key or button zooms. */
const keyStep = 12
const zoomStep = 0.1

/**
 * The person's header picture, framed by `framing`. The placement rule is
 * `placeHeaderImage`; this measures the header and the picture and writes the
 * result straight onto the image, so a resize or a drag moves pixels in the
 * same frame rather than a render later.
 *
 * While `adjusting`, the picture is dragged to move it, scrolled or pinched to
 * zoom, and moved with the arrow keys and + and −; a toolbar offers a zoom
 * slider, Reset, and Done. Enter keeps the framing and Escape abandons it.
 */
export function HeaderPicture({
  url,
  framing,
  adjusting,
  onFramingChange,
  onDone,
  onCancel,
}: {
  url: string
  framing: HeaderFraming
  adjusting: boolean
  onFramingChange: (next: HeaderFraming) => void
  onDone: () => void
  onCancel: () => void
}) {
  const frameRef = useRef<HTMLDivElement>(null)
  const imageRef = useRef<HTMLImageElement>(null)
  const framingRef = useRef(framing)
  framingRef.current = framing

  const sizes = () => {
    const frame = frameRef.current
    const image = imageRef.current
    if (!frame || !image) return null
    return {
      frame: { width: frame.clientWidth, height: frame.clientHeight },
      picture: { width: image.naturalWidth, height: image.naturalHeight },
    }
  }

  const place = () => {
    const measured = sizes()
    const image = imageRef.current
    if (!measured || !image) return
    const placed = placeHeaderImage(framingRef.current, measured.frame, measured.picture)
    image.style.width = `${placed.width}px`
    image.style.height = `${placed.height}px`
    image.style.transform = `translate(${placed.left}px, ${placed.top}px)`
  }

  useLayoutEffect(place)

  useEffect(() => {
    const frame = frameRef.current
    if (!frame) return
    const observer = new ResizeObserver(place)
    observer.observe(frame)
    return () => observer.disconnect()
    // `place` reads everything it needs through refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const change = (next: HeaderFraming) => {
    const measured = sizes()
    onFramingChange(
      measured ? clampHeaderFraming(next, measured.frame, measured.picture) : next,
    )
  }

  const pan = (dx: number, dy: number) => {
    const measured = sizes()
    if (!measured) return
    onFramingChange(
      panHeaderFraming(framingRef.current, dx, dy, measured.frame, measured.picture),
    )
  }

  const zoomBy = (delta: number) =>
    change({ ...framingRef.current, zoom: framingRef.current.zoom + delta })

  // Wheel and pinch zoom need a listener that may cancel the page's scroll.
  useEffect(() => {
    const frame = frameRef.current
    if (!frame || !adjusting) return
    const wheel = (event: WheelEvent) => {
      event.preventDefault()
      const rate = event.ctrlKey ? 0.01 : 0.002
      const current = framingRef.current
      change({ ...current, zoom: current.zoom * Math.exp(-event.deltaY * rate) })
    }
    frame.addEventListener("wheel", wheel, { passive: false })
    return () => frame.removeEventListener("wheel", wheel)
    // `change` reads everything it needs through refs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [adjusting])

  // On the next frame, after whatever opened adjusting has settled its focus.
  useEffect(() => {
    if (!adjusting) return
    const frame = requestAnimationFrame(() => frameRef.current?.focus())
    return () => cancelAnimationFrame(frame)
  }, [adjusting])

  return (
    <>
      <div
        ref={frameRef}
        className="desktop-header-image"
        data-adjusting={adjusting ? "" : undefined}
        role={adjusting ? "application" : undefined}
        aria-label={adjusting ? "Picture position" : undefined}
        aria-roledescription={
          adjusting ? "Drag or use the arrow keys to move, + and − to zoom" : undefined
        }
        aria-hidden={adjusting ? undefined : true}
        tabIndex={adjusting ? 0 : undefined}
        onPointerDown={(event) => {
          if (!adjusting || event.button !== 0) return
          event.currentTarget.setPointerCapture(event.pointerId)
          event.currentTarget.dataset.dragging = ""
        }}
        onPointerMove={(event) => {
          if (!adjusting || !event.currentTarget.hasPointerCapture(event.pointerId))
            return
          pan(event.movementX, event.movementY)
        }}
        onPointerUp={(event) => {
          delete event.currentTarget.dataset.dragging
        }}
        onPointerCancel={(event) => {
          delete event.currentTarget.dataset.dragging
        }}
        onKeyDown={(event) => {
          if (!adjusting) return
          const moves: Record<string, [number, number]> = {
            ArrowLeft: [keyStep, 0],
            ArrowRight: [-keyStep, 0],
            ArrowUp: [0, keyStep],
            ArrowDown: [0, -keyStep],
          }
          if (Object.hasOwn(moves, event.key)) {
            event.preventDefault()
            pan(...moves[event.key])
          } else if (event.key === "+" || event.key === "=") {
            event.preventDefault()
            zoomBy(zoomStep)
          } else if (event.key === "-") {
            event.preventDefault()
            zoomBy(-zoomStep)
          } else if (event.key === "Enter") {
            event.preventDefault()
            onDone()
          } else if (event.key === "Escape") {
            event.preventDefault()
            event.stopPropagation()
            onCancel()
          }
        }}
      >
        <img ref={imageRef} src={url} alt="" draggable={false} onLoad={place} />
      </div>
      {/* Outside the picture's fade, so it stays crisp. */}
      {adjusting ? (
        <div
          className="desktop-header-toolbar"
          role="toolbar"
          aria-label="Picture position"
        >
          <IconButton
            icon="zoomOut"
            label="Zoom out"
            shape="pill"
            tone="ink"
            onClick={() => zoomBy(-zoomStep)}
          />
          <input
            type="range"
            className="desktop-header-zoom"
            aria-label="Zoom"
            min={headerZoomRange.min}
            max={headerZoomRange.max}
            step={0.01}
            value={framing.zoom}
            onChange={(event) =>
              change({ ...framingRef.current, zoom: Number(event.target.value) })
            }
          />
          <IconButton
            icon="zoomIn"
            label="Zoom in"
            shape="pill"
            tone="ink"
            onClick={() => zoomBy(zoomStep)}
          />
          <span className="desktop-header-toolbar-divider" aria-hidden="true" />
          <button
            type="button"
            className="desktop-header-text-button"
            onClick={() => change(defaultHeaderFraming)}
          >
            Reset
          </button>
          <button
            type="button"
            className="desktop-header-text-button"
            data-primary=""
            onClick={onDone}
          >
            Done
          </button>
        </div>
      ) : null}
    </>
  )
}
