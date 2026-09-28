/**
 * What a drop on a pane does, as a value, before anything is dropped: the
 * zone the pointer is in, and the layout the drop would leave — the one the
 * drag previews and the one the command commits, from this one function, so
 * the preview is never a guess at the drop (`usecases/panes.ts` calls it for
 * both `movePane` and `dropSession`).
 *
 * | carried            | zone   | outcome                                          |
 * | ------------------ | ------ | ------------------------------------------------ |
 * | a pane             | middle | the two swap places                              |
 * | a pane             | side   | it moves there, if the room allows (`arrange`)   |
 * | a session          | middle | it takes the pane's place                        |
 * | a session          | side   | a new pane shows it there, if the room allows    |
 * | a session on screen| any    | its pane is focused where it is                  |
 * | anything           | —      | nothing, where the room refuses: no zone offered |
 */
import {
  focusPane,
  movePane,
  paneShowing,
  showInPane,
  splitPane,
  type PaneKey,
  type PaneLayout,
  type Side,
  type Zone,
} from "./pane-layout"
import { arrange, type Arranged, type WorkspaceRoom } from "./pane-sizing"

/** What is carried: a pane by its header, or a session from a list. */
export type Carried =
  | { readonly kind: "pane"; readonly pane: PaneKey }
  | { readonly kind: "session"; readonly sessionId: string }

/**
 * Where on a pane a drop lands, in the pane's own pixels, as VS Code's
 * editor groups divide theirs:
 *
 * ```text
 *   ┌──────────────────────┐
 *   │ ╲        top       ╱ │   each side is the triangle between the
 *   │   ╲──────────────╱   │   pane's diagonals — the side the pointer is
 *   │ l  │   center   │  r │   nearest in pixels, so a tall narrow pane's
 *   │ e  │  (replace) │  i │   top and bottom are short, not most of it —
 *   │ f  │            │  g │   and a centre `centreInset` in from every
 *   │ t  │            │  h │   edge, a size in pixels, not a share.
 *   │   ╱──────────────╲ t │
 *   │ ╱      bottom      ╲ │
 *   └──────────────────────┘
 * ```
 *
 * The pointer's heading weighs the sides within its reach: travelling toward
 * one brings it nearer, away sends it further (`headingPull`), so a sideways
 * sweep across a pane's top corner reaches the side, not the top — and a side
 * out of reach stays out of it, however the pointer heads. And the zone it is in holds:
 * another takes over only once it wins by `zoneHold` pixels, so the zone
 * does not flicker at a boundary.
 */
export interface Pointer {
  /** From the pane's top left, in pixels. */
  readonly x: number
  readonly y: number
  /** How it is moving, in pixels per millisecond (`pointerVelocity`). */
  readonly velocity: { readonly x: number; readonly y: number }
}

/** How far in from every edge the centre begins: its share of the pane's shorter side, held to 48–120px. */
export function centreInset(size: { width: number; height: number }): number {
  return Math.min(Math.max(0.28 * Math.min(size.width, size.height), 48), 120)
}

/** How much a side the pointer heads straight for is brought nearer, as a share of its distance. */
export const headingPull = 0.5

/** Below this speed, in px/ms, the pointer is resting: its heading says nothing. */
export const restingSpeed = 0.05

/** By how many pixels another zone must win before it takes over from the one the pointer is in. */
export const zoneHold = 12

const outward: Record<Side, { x: number; y: number }> = {
  left: { x: -1, y: 0 },
  right: { x: 1, y: 0 },
  top: { x: 0, y: -1 },
  bottom: { x: 0, y: 1 },
}

/** The zone of a pane of `size` the pointer is in; `now` is the zone it was in over this pane, if any. */
export function zoneAt(
  pointer: Pointer,
  size: { readonly width: number; readonly height: number },
  now: Zone | null = null,
): Zone {
  const distance: Record<Side, number> = {
    left: pointer.x,
    right: size.width - pointer.x,
    top: pointer.y,
    bottom: size.height - pointer.y,
  }
  const sides = Object.keys(distance) as Side[]
  const nearest = Math.min(...sides.map((side) => distance[side]))
  const inset = centreInset(size)
  // The centre's edge holds too: in it, the pointer leaves only well past it; outside, it enters only well within.
  const centre =
    now === "center" ? nearest >= inset - zoneHold : nearest >= inset + zoneHold
  if (centre || (now === null && nearest >= inset)) return "center"
  const speed = Math.hypot(pointer.velocity.x, pointer.velocity.y)
  const weighed = (side: Side) => {
    const heading =
      speed < restingSpeed
        ? 0
        : (pointer.velocity.x * outward[side].x + pointer.velocity.y * outward[side].y) /
          speed
    return distance[side] * (1 - headingPull * heading) - (side === now ? zoneHold : 0)
  }
  // Only a side the pointer is within reach of: heading for the far side of
  // a pane does not make it nearer than the side the pointer is at.
  const within = sides.filter((side) => distance[side] < inset + zoneHold)
  return within.reduce((best, side) => (weighed(side) < weighed(best) ? side : best))
}

/** A pointer position at a moment, in pixels and milliseconds. */
export interface PointerSample {
  readonly x: number
  readonly y: number
  readonly t: number
}

/** How far back the pointer's heading is read, in milliseconds. */
export const headingWindow = 100

/**
 * How the pointer is moving, in pixels per millisecond: from the oldest
 * sample within `headingWindow` of the newest to the newest. Fewer than two
 * samples, or none apart in time, is resting.
 */
export function pointerVelocity(samples: readonly PointerSample[]): {
  x: number
  y: number
} {
  const last = samples.at(-1)
  if (!last) return { x: 0, y: 0 }
  const first = samples.find((sample) => last.t - sample.t <= headingWindow) ?? last
  const span = last.t - first.t
  return span > 0
    ? { x: (last.x - first.x) / span, y: (last.y - first.y) / span }
    : { x: 0, y: 0 }
}

/** What a drop leaves: the layout, whether the sidebar folds for it, and the pane it lands in. */
export interface DropOutcome extends Arranged {
  /** The pane that shows what was carried once it lands. */
  readonly lands: PaneKey
  /** What the drop does, in words a person would use of it. */
  readonly does: "swap" | "move" | "replace" | "split" | "go-to"
}

/**
 * The layout a drop on `zone` of `target` leaves, in `room`; `null` where
 * nothing would happen — a pane dropped on itself, or a side the room
 * refuses.
 */
export function dropOutcome(
  layout: PaneLayout,
  carried: Carried,
  target: PaneKey,
  zone: Zone,
  room: WorkspaceRoom | undefined,
): DropOutcome | null {
  if (carried.kind === "pane") {
    const placed = arrange(layout, movePane(layout, carried.pane, target, zone), room)
    return placed
      ? { ...placed, lands: carried.pane, does: zone === "center" ? "swap" : "move" }
      : null
  }
  const shown = paneShowing(layout, carried.sessionId)
  if (shown)
    return {
      layout: focusPane(layout, shown.key),
      foldSidebar: false,
      lands: shown.key,
      does: "go-to",
    }
  if (zone === "center") {
    const shownThere = showInPane(layout, target, carried.sessionId)
    return shownThere === layout
      ? null
      : { layout: shownThere, foldSidebar: false, lands: target, does: "replace" }
  }
  const placed = arrange(layout, splitPane(layout, target, zone, carried.sessionId), room)
  // A new pane is minted the layout's next key.
  return placed ? { ...placed, lands: layout.nextKey, does: "split" } : null
}
