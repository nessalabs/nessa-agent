
/**
 * The opening of the setup window, as beats on one clock.
 *
 * A glow gathers on the desktop, spreads past every edge of the screen, then
 * draws itself in until it is the window, and only then does the window open
 * its pane. Each beat is a moment the light is handed a new target; how it
 * travels there is the light's own business, so the timing lives here once and
 * nothing else keeps a copy of it.
 */
export type OpeningBeat =
  /** A small light gathers on the desktop. No window yet. */
  | "glow"
  /** The light spreads past every edge of the screen. */
  | "fill"
  /** The light draws itself in, sharpens into the window and gains its shadow. */
  | "form"
  /** The window is there: its pane and controls come in. */
  | "open"

/** When each beat starts, in milliseconds from the first frame, in order. */
export const OPENING_BEATS: readonly { beat: OpeningBeat; at: number }[] = Object.freeze([
  Object.freeze({ beat: "glow" as const, at: 0 }),
  Object.freeze({ beat: "fill" as const, at: 1000 }),
  Object.freeze({ beat: "form" as const, at: 2000 }),
  Object.freeze({ beat: "open" as const, at: 2900 }),
])

/** The beat the opening is on `elapsed` ms after it started. */
export function openingBeat(elapsed: number): OpeningBeat {
  let current: OpeningBeat = "glow"
  for (const { beat, at } of OPENING_BEATS) if (elapsed >= at) current = beat
  return current
}

/** The beat after this one, or the same beat when it is the last. */
export function nextBeat(beat: OpeningBeat): OpeningBeat {
  const index = OPENING_BEATS.findIndex((entry) => entry.beat === beat)
  return (OPENING_BEATS[index + 1] ?? OPENING_BEATS[index]).beat
}

/** How long a beat lasts before the next one, or undefined for the last. */
export function beatLength(beat: OpeningBeat): number | undefined {
  const index = OPENING_BEATS.findIndex((entry) => entry.beat === beat)
  const next = OPENING_BEATS[index + 1]
  return next ? next.at - OPENING_BEATS[index].at : undefined
}

/**
 * A value the light travels to, in the shape the field takes: a number is
 * where it is at once, a tween says where it goes and how.
 */
export type FieldValue =
  | number
  | {
      to: number
      from?: number
      delay?: number
      duration?: number
      easing?: "out" | "in-out"
    }

/** What the window's light is told to do. */
export interface SetupField {
  spread: FieldValue
  /** How much of the ground is see-through: 1 while it is a glow on the desktop. */
  clear: FieldValue
  /** Where the colour is centred across the window. */
  x: FieldValue
}

/**
 * What shows where no colour reaches. Light, because the window is light: a
 * dark ground showed through as a dark smudge wherever the blooms happened
 * not to cover, which reads as the dimmed backdrop this opening has none of.
 */
export const FIELD_GROUND = "#e6dcf3"

/** The agents the picker can show on its card: the two it offers, and the
 * Local AI that is listed but cannot be chosen yet. */
export type FigureId = "claude" | "codex" | "local"

/**
 * What the light does for where setup is.
 *
 * Through the opening it is a glow on a see-through ground that floods and
 * then comes in to the window's shape; once the window is open its colour sits
 * to the right, behind the picture, with the pane over the rest. It is the
 * same on every step: the picker's agent stands on its own card, not on the
 * light.
 */
export function setupField(beat: OpeningBeat): SetupField {
  if (beat === "glow") {
    return { spread: { from: 0, to: 0.08, duration: 1000, easing: "out" }, clear: 1, x: 0.5 }
  }
  if (beat === "fill") return { spread: { to: 1.25, duration: 1000 }, clear: 1, x: 0.5 }
  if (beat === "form") {
    return {
      spread: { to: 1, duration: 900 },
      clear: { to: 0, delay: 400, duration: 500 },
      x: 0.5,
    }
  }
  return { spread: 1, clear: 0, x: { to: 0.72, duration: 300, easing: "out" } }
}

/**
 * How much larger than itself the window has to be drawn to cover the screen,
 * corners and all, while it is still a glow.
 *
 * The window is rounded, so covering the screen edge to edge is not enough:
 * its corners would show. The margin carries them past the screen's own.
 */
export function coverScale(
  screen: { width: number; height: number },
  window: { width: number; height: number },
): number {
  if (window.width <= 0 || window.height <= 0) return 1
  return 1.3 * Math.max(screen.width / window.width, screen.height / window.height, 1)
}

/**
 * Where the finished window flies to, in the window's own coordinates.
 *
 * `tray` is the menu bar icon's centre as the host reports it, already in the
 * page's CSS pixels. A host that cannot say (no tray, or a system that does not
 * report where it is) gets the top right corner, which is where a menu bar's
 * status icons are on every desktop this runs on.
 */
export function flightTarget(
  tray: { x: number; y: number } | undefined,
  screen: { width: number; height: number },
): { x: number; y: number } {
  if (tray && Number.isFinite(tray.x) && Number.isFinite(tray.y)) return tray
  return { x: Math.max(0, screen.width - 120), y: 12 }
}

/** How long the finished window takes to gather and fly into the menu bar. */
export const FLIGHT_MS = 900

/** The size of the light the window gathers into before it flies: about the
 * size of a menu bar icon, so what lands is the icon's size. */
export const FLIGHT_LIGHT_PX = 22

/**
 * The finished window's last movement, as keyframes for the window itself.
 *
 * It gathers in place into a small circle first, then flies to `target` and
 * fades as it lands. The circle is the window's own box with fully rounded
 * corners scaled to a square, which is a circle; scaling a rectangle evenly
 * would land an ellipse.
 */
export function flightFrames(
  window: { left: number; top: number; width: number; height: number },
  target: { x: number; y: number },
): Keyframe[] {
  if (window.width <= 0 || window.height <= 0) return []
  const sx = FLIGHT_LIGHT_PX / window.width
  const sy = FLIGHT_LIGHT_PX / window.height
  const dx = target.x - (window.left + window.width / 2)
  const dy = target.y - (window.top + window.height / 2)
  const gathered = `scale(${sx}, ${sy})`
  const landed = `translate(${dx}px, ${dy}px) scale(${sx * 0.8}, ${sy * 0.8})`
  return [
    {
      offset: 0,
      transform: "none",
      borderRadius: "22px",
      opacity: 1,
      easing: "cubic-bezier(0.4, 0, 0.2, 1)",
    },
    {
      offset: 0.45,
      transform: gathered,
      borderRadius: "50%",
      opacity: 1,
      easing: "cubic-bezier(0.5, 0, 0.2, 1)",
    },
    { offset: 0.92, transform: landed, borderRadius: "50%", opacity: 1 },
    { offset: 1, transform: landed, borderRadius: "50%", opacity: 0 },
  ]
}
