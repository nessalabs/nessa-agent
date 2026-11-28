/**
 * How large the window draws everything: its zoom, as a browser's page zoom
 * is. One number scales the whole page (type, icons, rows, spacing, every
 * surface), so nothing in the window keeps a size of its own to scale.
 * Stepped with ⌘= and ⌘-, and put back to 100% with ⌘0
 * (`adapters/window-zoom.ts`).
 *
 * The zoom is always one of `zoomSteps`: a stored or requested value that is
 * not one reads as `DEFAULT_ZOOM`. The steps are `src/host/window-zoom.json`,
 * the one list the host reads too (`src-tauri/src/desktop_window.rs`) and
 * refuses anything outside of, so the page and the host cannot come to
 * disagree on a factor neither can step back from.
 */
import zoom from "../../host/window-zoom.json"

/** One of `zoomSteps`: what `parseZoom` and the steps return. */
export type WindowZoom = number

/** Every zoom the window takes, smallest first. */
export const zoomSteps: readonly WindowZoom[] = zoom.steps

export const DEFAULT_ZOOM: WindowZoom = zoom.default

/** Reads a stored or requested zoom: a number or its string, and only a known step. */
export function parseZoom(value: unknown): WindowZoom {
  const factor = typeof value === "string" && value.trim() !== "" ? Number(value) : value
  return zoomSteps.find((step) => step === factor) ?? DEFAULT_ZOOM
}

/** Where a zoom stands among the steps; one that is not a step stands at the default. */
const at = (zoom: WindowZoom) =>
  zoomSteps.indexOf(zoomSteps.includes(zoom) ? zoom : DEFAULT_ZOOM)

/** The next step larger, or the largest. */
export function zoomedIn(zoom: WindowZoom): WindowZoom {
  return zoomSteps[Math.min(at(zoom) + 1, zoomSteps.length - 1)]
}

/** The next step smaller, or the smallest. */
export function zoomedOut(zoom: WindowZoom): WindowZoom {
  return zoomSteps[Math.max(at(zoom) - 1, 0)]
}

/** The zoom as a person reads it: "110%". */
export function zoomPercent(zoom: WindowZoom): string {
  return `${Math.round(zoom * 100)}%`
}
