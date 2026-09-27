/**
 * What the home header shows: the night scene, or a picture the person chose.
 * A picture is any image the webview can draw, animated GIFs included, up to
 * `headerImageMaxBytes`; anything else is refused with a typed reason.
 */
export const headerImageMaxBytes = 25 * 1024 * 1024

export type HeaderImageRefusal = "not-an-image" | "too-large"

export type HeaderImageCheck = { ok: true } | { ok: false; reason: HeaderImageRefusal }

/** Whether a chosen file can become the header picture. */
export function checkHeaderImage(file: { type: string; size: number }): HeaderImageCheck {
  if (!file.type.startsWith("image/")) return { ok: false, reason: "not-an-image" }
  if (file.size > headerImageMaxBytes) return { ok: false, reason: "too-large" }
  return { ok: true }
}

/** What to tell the person about a refused file. */
export const headerImageRefusalText: Record<HeaderImageRefusal, string> = {
  "not-an-image": "That file isn’t an image",
  "too-large": "Images up to 25 MB",
}

/**
 * How a picture is framed in the header: `x` and `y` are the point of the
 * picture, as fractions of its width and height, that sits at the header's
 * centre, and `zoom` is how far past covering the header it is enlarged.
 */
export interface HeaderFraming {
  x: number
  y: number
  zoom: number
}

export const headerZoomRange = { min: 1, max: 3 } as const

export const defaultHeaderFraming: HeaderFraming = { x: 0.5, y: 0.5, zoom: 1 }

type Size = { width: number; height: number }

/** Where the picture's box goes in the header, in pixels from the header's corner. */
export interface HeaderPlacement {
  width: number
  height: number
  left: number
  top: number
}

function within(value: number, low: number, high: number) {
  return low > high ? (low + high) / 2 : Math.min(high, Math.max(low, value))
}

/**
 * Keeps a framing possible: the zoom within range, and the focal point where
 * the picture still covers the whole header, so no edge ever shows a gap.
 */
export function clampHeaderFraming(
  framing: HeaderFraming,
  frame: Size,
  picture: Size,
): HeaderFraming {
  const zoom = within(framing.zoom, headerZoomRange.min, headerZoomRange.max)
  const { width, height } = placeHeaderImage({ ...framing, zoom }, frame, picture, false)
  if (width <= 0 || height <= 0) return { ...defaultHeaderFraming, zoom }
  const halfX = frame.width / 2 / width
  const halfY = frame.height / 2 / height
  return {
    x: within(framing.x, halfX, 1 - halfX),
    y: within(framing.y, halfY, 1 - halfY),
    zoom,
  }
}

/**
 * Sizes the picture to cover the header and enlarges it by the zoom, then
 * places it so the framing's focal point sits at the header's centre.
 */
export function placeHeaderImage(
  framing: HeaderFraming,
  frame: Size,
  picture: Size,
  clamp = true,
): HeaderPlacement {
  if (picture.width <= 0 || picture.height <= 0) {
    return { width: frame.width, height: frame.height, left: 0, top: 0 }
  }
  const f = clamp ? clampHeaderFraming(framing, frame, picture) : framing
  const cover = Math.max(frame.width / picture.width, frame.height / picture.height)
  const width = picture.width * cover * f.zoom
  const height = picture.height * cover * f.zoom
  return {
    width,
    height,
    left: frame.width / 2 - f.x * width,
    top: frame.height / 2 - f.y * height,
  }
}

/**
 * Moves the framing by a drag of `dx`, `dy` pixels: the picture follows the
 * pointer, so its focal point moves the other way.
 */
export function panHeaderFraming(
  framing: HeaderFraming,
  dx: number,
  dy: number,
  frame: Size,
  picture: Size,
): HeaderFraming {
  const { width, height } = placeHeaderImage(framing, frame, picture)
  if (width <= 0 || height <= 0) return framing
  return clampHeaderFraming(
    { ...framing, x: framing.x - dx / width, y: framing.y - dy / height },
    frame,
    picture,
  )
}

/** Reads a stored framing, falling back to the default for anything malformed. */
export function parseHeaderFraming(value: unknown): HeaderFraming {
  if (typeof value !== "object" || value === null) return defaultHeaderFraming
  const { x, y, zoom } = value as Record<string, unknown>
  const finite = (n: unknown): n is number => typeof n === "number" && Number.isFinite(n)
  if (!finite(x) || !finite(y) || !finite(zoom)) return defaultHeaderFraming
  return {
    x: within(x, 0, 1),
    y: within(y, 0, 1),
    zoom: within(zoom, headerZoomRange.min, headerZoomRange.max),
  }
}
