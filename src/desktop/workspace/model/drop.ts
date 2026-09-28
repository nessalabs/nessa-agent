/**
 * What a drop on a pane does, as a value, before anything is dropped: where
 * a drag aims (`aimAt`, the pointer itself), the zone it is in, and the
 * layout the drop would leave — the one the
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
 *   │ ╲        top       ╱ │   each side reaches a share of the way in —
 *   │   ╲──────────────╱   │   a third of the pane across it, held to
 *   │ l  │   center   │  r │   90–300px (`edgeReach`) — the middle is what
 *   │ e  │  (replace) │  i │   is left; where two sides' reaches meet, the
 *   │ f  │            │  g │   diagonal between them decides, as a share of
 *   │ t  │            │  h │   each reach.
 *   │   ╱──────────────╲ t │
 *   │ ╱      bottom      ╲ │
 *   └──────────────────────┘
 * ```
 *
 * The pointer's heading over the last tenth of a second
 * (`pointerVelocity`) reads its intent early and alike in every direction:
 *
 * | the pointer heads            | the side it heads for         | the sides across its way            |
 * | ---------------------------- | ----------------------------- | ----------------------------------- |
 * | mostly toward a side         | reaches `headingReach` times further, so moving down a pane from its middle is "below" by two-thirds of the way | as at rest                          |
 * | plainly along one axis       | as above                      | only within `edgeHug` of their edge, unless already in one |
 * | resting, or still for `restAfter` | as at rest               | as at rest                          |
 *
 * so a sideways drag near a tall narrow pane's top moves beside it, never
 * above, and a drag down a tall pane splits below well before its foot. The
 * zone the pointer is in holds: another takes over only once it wins by
 * `zoneHold` pixels, so the zone does not flicker at a boundary.
 */
export interface Pointer {
  /** From the pane's top left, in pixels. */
  readonly x: number
  readonly y: number
  /** How it is moving, in pixels per millisecond (`pointerVelocity`). */
  readonly velocity: { readonly x: number; readonly y: number }
}

/** A side's reach into the pane: its share of the pane across it, held to a floor and a cap. */
export const edgeShare = 1 / 3
export const edgeFloor = 90
export const edgeCap = 300

/** How much further a side reaches while the pointer heads mostly toward it. */
export const headingReach = 1.4

/** How much of the pointer's motion must be toward a side for it to be heading there (a cosine). */
export const toward = 0.7

/** Below this speed, in px/ms, the pointer is resting: its heading says nothing. */
export const restingSpeed = 0.05

/** By how many pixels another zone must win before it takes over from the one the pointer is in. */
export const zoneHold = 12

/** How many times more along one axis than the other a heading must be to be plainly that way. */
export const plainly = 2

/** How near an edge, in pixels, the pointer must be for a heading across it to still reach it. */
export const edgeHug = 16

const outward: Record<Side, { x: number; y: number }> = {
  left: { x: -1, y: 0 },
  right: { x: 1, y: 0 },
  top: { x: 0, y: -1 },
  bottom: { x: 0, y: 1 },
}

const across: Record<Side, "x" | "y"> = { left: "x", right: "x", top: "y", bottom: "y" }

/**
 * How far into a pane of `size` a side reaches, at rest: a share of the pane
 * across it, held to 90–300px, and never past its middle.
 */
export function edgeReach(
  size: { readonly width: number; readonly height: number },
  side: Side,
): number {
  const extent = across[side] === "x" ? size.width : size.height
  const reach = Math.min(Math.max(edgeShare * extent, edgeFloor), edgeCap)
  return Math.min(reach, extent / 2)
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
  const { x: vx, y: vy } = pointer.velocity
  const speed = Math.hypot(vx, vy)
  const moving = speed >= restingSpeed
  const headingTo = (side: Side) =>
    moving ? (vx * outward[side].x + vy * outward[side].y) / speed : 0
  // Plainly along one axis: the sides across it are reached only at their edge.
  const plain = !moving
    ? null
    : Math.abs(vx) >= plainly * Math.abs(vy)
      ? "x"
      : Math.abs(vy) >= plainly * Math.abs(vx)
        ? "y"
        : null
  const reach = (side: Side) => {
    const extent = across[side] === "x" ? size.width : size.height
    const heading = headingTo(side) >= toward ? headingReach : 1
    return Math.min(edgeReach(size, side) * heading, extent / 2)
  }
  // The zone the pointer is in holds by `zoneHold` pixels: its side reaches
  // that much further, and — resting in the middle — the sides that much less.
  const hold = (side: Side) =>
    side === now ? zoneHold : now === "center" ? -zoneHold : 0
  // A plain heading keeps the pointer out of the sides across it, but does not
  // throw it out of the one it is in: jitter across a boundary holds.
  const candidates = sides.filter(
    (side) =>
      distance[side] < reach(side) + hold(side) &&
      (plain === null ||
        across[side] === plain ||
        side === now ||
        distance[side] <= edgeHug),
  )
  if (candidates.length === 0) return "center"
  // Where two reaches meet, the diagonal between them, as a share of each;
  // the side the pointer is in wins ties by `zoneHold` pixels.
  const depth = (side: Side) =>
    (distance[side] - (side === now ? zoneHold : 0)) / reach(side)
  return candidates.reduce((best, side) => (depth(side) < depth(best) ? side : best))
}

/** A box on the page, in pixels. */
export interface Rect {
  readonly left: number
  readonly top: number
  readonly width: number
  readonly height: number
}

/**
 * The pane a point in the grid is over, and where on it: the pane the point
 * is in; else — the point in a gutter between panes — the nearest pane, the
 * point held to its edge. `null` only with no panes.
 */
export function paneAt(
  point: { readonly x: number; readonly y: number },
  panes: Iterable<readonly [PaneKey, Rect]>,
): { key: PaneKey; x: number; y: number; box: Rect } | null {
  let nearest: { key: PaneKey; x: number; y: number; box: Rect; off: number } | null =
    null
  for (const [key, box] of panes) {
    const x = Math.min(Math.max(point.x - box.left, 0), box.width)
    const y = Math.min(Math.max(point.y - box.top, 0), box.height)
    const off = Math.hypot(point.x - box.left - x, point.y - box.top - y)
    if (!nearest || off < nearest.off) nearest = { key, x, y, box, off }
  }
  return nearest && { key: nearest.key, x: nearest.x, y: nearest.y, box: nearest.box }
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
 * How long the pointer may stay where it is, in milliseconds, before its
 * heading ages out and the zone is decided as at rest.
 */
export const restAfter = 150

/**
 * How the pointer is moving at `now`, in pixels per millisecond: from the
 * oldest sample within `headingWindow` of the newest to the newest. Fewer than
 * two samples, none apart in time, or no move for `restAfter` is resting.
 */
export function pointerVelocity(
  samples: readonly PointerSample[],
  now: number,
): { x: number; y: number } {
  const last = samples.at(-1)
  if (!last || now - last.t >= restAfter) return { x: 0, y: 0 }
  const first = samples.find((sample) => last.t - sample.t <= headingWindow) ?? last
  const span = last.t - first.t
  return span > 0
    ? { x: (last.x - first.x) / span, y: (last.y - first.y) / span }
    : { x: 0, y: 0 }
}

/**
 * What a drag may aim at: the grid and every pane in it, as laid out when the
 * press began — or `null` while no pane can be seen (the Agents overview or
 * Settings over them).
 */
export interface Targets {
  readonly grid: Rect
  readonly panes: readonly (readonly [PaneKey, Rect])[]
  /**
   * The side columns drawn as the press began — docked beside the grid, or
   * revealed from the window's edge over it: never a target, whatever is
   * under them.
   */
  readonly covered: readonly Rect[]
}

const within = (point: { readonly x: number; readonly y: number }, box: Rect) =>
  point.x >= box.left &&
  point.x <= box.left + box.width &&
  point.y >= box.top &&
  point.y <= box.top + box.height

/**
 * Whether a drag can aim where the pointer is at all: a pane in sight, and
 * the pointer on the grid and off every side column.
 */
export function inReach(
  point: { readonly x: number; readonly y: number },
  targets: Targets | null,
): targets is Targets {
  return (
    targets !== null &&
    within(point, targets.grid) &&
    !targets.covered.some((column) => within(point, column))
  )
}

/** The pane and zone a drag aims at. */
export interface Aim {
  readonly target: PaneKey
  readonly zone: Zone
}

/**
 * Where a drag aims: the pointer, and nothing else — what the eye follows
 * (the copy's centre, `model/drag.ts`) and what aims are one point. Off the
 * grid, over a side column, or with no pane in sight, nothing; in a gutter,
 * the nearest pane
 * (`paneAt`); on a pane, its zone (`zoneAt`), weighed by where the pointer
 * heads at `now` (`pointerVelocity`) and held while it stays on the pane it
 * was aiming at (`was`).
 */
export function aimAt(
  path: readonly PointerSample[],
  now: number,
  was: Aim | null,
  targets: Targets | null,
): Aim | null {
  const pointer = path.at(-1)
  if (!pointer || !inReach(pointer, targets)) return null
  const over = paneAt(pointer, targets.panes)
  if (!over) return null
  return {
    target: over.key,
    zone: zoneAt(
      { x: over.x, y: over.y, velocity: pointerVelocity(path, now) },
      over.box,
      was?.target === over.key ? was.zone : null,
    ),
  }
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
