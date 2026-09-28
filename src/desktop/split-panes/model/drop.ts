/**
 * What a drop on a pane does, as a value, before anything is dropped: where
 * a drag aims (`aimAt`, the pointer itself), the zone it is in, and the
 * layout the drop would leave — the one the
 * drag previews and the one the command commits, from this one function, so
 * the preview is never a guess at the drop (the workspace's `usecases/panes.ts`
 * calls it for both `movePane` and `dropSession`).
 *
 * | carried            | zone   | outcome                                          |
 * | ------------------ | ------ | ------------------------------------------------ |
 * | a pane             | middle | the two swap places                              |
 * | a pane             | side   | it moves there, if the room allows (`arrange`)   |
 * | an item            | middle | it takes the pane's place                        |
 * | an item            | side   | a new pane shows it there, if the room allows    |
 * | an item on screen  | any    | its pane is focused where it is                  |
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
import { arrange, placements, type Arranged, type PaneRoom } from "./pane-sizing"

/** What is carried: a pane by its header, or an item from outside the grid (a session from a list). */
export type Carried =
  | { readonly kind: "pane"; readonly pane: PaneKey }
  | { readonly kind: "item"; readonly item: string }

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
 * zone the pointer is in holds: resting or settling (`settlingSpeed`), its
 * side keeps the reach a heading gave it, and another takes over only once
 * it wins by
 * `zoneHold` pixels, so the zone flickers neither at a boundary nor as the
 * hand stops. A zone a drop there would change nothing on — the side of a
 * pane the carried one already sits on — or one the room refuses is never
 * aimed at (`refusedZones`): its ground is the middle's, but near a corner
 * the side beside it takes it (`cornerReach`).
 */
export interface Pointer {
  /** From the pane's top left, in pixels. */
  readonly x: number
  readonly y: number
  /** How it is moving, in pixels per millisecond (`pointerVelocity`). */
  readonly velocity: { readonly x: number; readonly y: number }
  /** How far it has come since the drag was pressed, in pixels: which way the pane is being moved. */
  readonly travel: { readonly x: number; readonly y: number }
}

/** A side's reach into the pane: its share of the pane across it, held to a floor and a cap. */
export const edgeShare = 1 / 3
export const edgeFloor = 90
export const edgeCap = 300

/** How much further a side reaches while the pointer heads mostly toward it. */
export const headingReach = 1.4

/**
 * How far, in pixels, the pointer must have come since the press for the way
 * it came to count: a pane moved up reaches the top sides further, one moved
 * sideways the left or right, as a heading does.
 */
export const travelled = 24

/** How much of the pointer's motion must be toward a side for it to be heading there (a cosine). */
export const toward = 0.7

/** Below this speed, in px/ms, the pointer is resting: its heading says nothing. */
export const restingSpeed = 0.05

/**
 * Below this speed, in px/ms, the pointer is settling — a hand nudging into
 * place — and the side it is in keeps its reach; faster, it sweeps.
 */
export const settlingSpeed = 0.25

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

/**
 * The zone of a pane of `size` the pointer is in (`within`), and the one it
 * offers (`zone`): the same, unless that zone is refused — then the middle,
 * or near a corner the side beside it (`cornerReach`). `now` is the zone it
 * was in over this pane, if any: what holds, refused or not, so the pointer
 * moving about a refused side's ground stays on it.
 */
export function zonesAt(
  pointer: Pointer,
  size: { readonly width: number; readonly height: number },
  now: Zone | null = null,
  refused: ReadonlySet<Zone> = new Set(),
): { readonly within: Zone; readonly zone: Zone } {
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
  // Which way the pane has been moved since the press: moved up, the top
  // sides reach further; moved sideways, the left or right.
  const { x: tx, y: ty } = pointer.travel
  const distanceCome = Math.hypot(tx, ty)
  const cameToward = (side: Side) =>
    distanceCome >= travelled
      ? (tx * outward[side].x + ty * outward[side].y) / distanceCome
      : 0
  // Resting or settling, the side the pointer is in keeps the reach a
  // heading gives: coming to rest, or nudging into place, never throws it
  // out of the zone it moved into. Sweeping, only a heading toward a side
  // — or the way the pane has come — reaches further.
  const settling = speed < settlingSpeed
  const reach = (side: Side) => {
    const extent = across[side] === "x" ? size.width : size.height
    const heading =
      (side === now && settling) ||
      headingTo(side) >= toward ||
      cameToward(side) >= toward
        ? headingReach
        : 1
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
  // Where two reaches meet, the diagonal between them, as a share of each;
  // the side the pointer is in wins ties by `zoneHold` pixels.
  const depth = (side: Side) =>
    (distance[side] - (side === now ? zoneHold : 0)) / reach(side)
  const [nearest] = candidates.sort((a, b) => depth(a) - depth(b))
  const within: Zone = nearest ?? "center"
  if (!refused.has(within)) return { within, zone: within }
  // A refused zone's ground is the middle's — but near a corner, the side
  // beside it takes it: within `cornerReach` of its edge, however the
  // pointer heads, so moving along that edge does not flicker — unless the
  // pane has plainly come along the other axis: moved sideways, it is never
  // put above or below.
  const cameAlong =
    distanceCome < travelled
      ? null
      : Math.abs(tx) >= plainly * Math.abs(ty)
        ? "x"
        : Math.abs(ty) >= plainly * Math.abs(tx)
          ? "y"
          : null
  const [beside] = sides
    .filter(
      (side) =>
        !refused.has(side) &&
        distance[side] <= cornerReach &&
        (cameAlong === null || across[side] === cameAlong),
    )
    .sort((a, b) => distance[a] - distance[b])
  return { within, zone: beside ?? "center" }
}

/** The zone of a pane of `size` the pointer is offered (`zonesAt`). */
export const zoneAt = (
  pointer: Pointer,
  size: { readonly width: number; readonly height: number },
  now: Zone | null = null,
  refused: ReadonlySet<Zone> = new Set(),
): Zone => zonesAt(pointer, size, now, refused).zone

/** How near its edge, in pixels, the side beside a refused one takes the refused one's ground. */
export const cornerReach = 48

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
   * What the host draws over or beside the grid as the press began — the
   * workspace's side columns, docked or revealed from the window's edge
   * over it: never a target, whatever is under them.
   */
  readonly covered: readonly Rect[]
  /** Each pane's zones a drop would change nothing on, or the room refuses (`refusedZones`). */
  readonly refused: ReadonlyMap<PaneKey, ReadonlySet<Zone>>
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
  /** What a drop there does: never a zone the pane refuses. */
  readonly zone: Zone
  /** The zone the pointer is in, refused or not: what holds as it moves (`zonesAt`). */
  readonly within: Zone
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
  /** Where the drag was pressed: which way the pane has come. */
  from: PointerSample,
): Aim | null {
  const pointer = path.at(-1)
  if (!pointer || !inReach(pointer, targets)) return null
  const over = paneAt(pointer, targets.panes)
  if (!over) return null
  const refused = targets.refused.get(over.key) ?? noZones
  const { within, zone } = zonesAt(
    {
      x: over.x,
      y: over.y,
      velocity: pointerVelocity(path, now),
      travel: { x: pointer.x - from.x, y: pointer.y - from.y },
    },
    over.box,
    was?.target === over.key ? was.within : null,
    refused,
  )
  // A pane over itself, say, refuses every zone: nothing is aimed at.
  return refused.has(zone) ? null : { target: over.key, zone, within }
}

const noZones: ReadonlySet<Zone> = new Set()

/** What a drop leaves: the layout, whether it takes the host's spare room, and the pane it lands in. */
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
  room: PaneRoom | undefined,
): DropOutcome | null {
  if (carried.kind === "pane") {
    const placed = arrange(layout, movePane(layout, carried.pane, target, zone), room)
    return placed
      ? { ...placed, lands: carried.pane, does: zone === "center" ? "swap" : "move" }
      : null
  }
  const shown = paneShowing(layout, carried.item)
  if (shown)
    return {
      layout: focusPane(layout, shown.key),
      takesSpare: false,
      lands: shown.key,
      does: "go-to",
    }
  if (zone === "center") {
    const shownThere = showInPane(layout, target, carried.item)
    return shownThere === layout
      ? null
      : { layout: shownThere, takesSpare: false, lands: target, does: "replace" }
  }
  const placed = arrange(layout, splitPane(layout, target, zone, carried.item), room)
  // A new pane is minted the layout's next key.
  return placed ? { ...placed, lands: layout.nextKey, does: "split" } : null
}

const zones: readonly Zone[] = ["left", "right", "top", "bottom", "center"]

/**
 * Whether a drop's outcome leaves every pane where it was in `layout`: a pane
 * moved to the side of the one it already sits beside, or dropped on
 * itself. Focus aside, it would do nothing, so it is not offered.
 */
function movesNothing(layout: PaneLayout, outcome: DropOutcome): boolean {
  if ((outcome.does !== "move" && outcome.does !== "swap") || outcome.takesSpare)
    return false
  const before = placements(layout.columns).panes
  const after = new Map(
    placements(outcome.layout.columns).panes.map((placement) => [
      placement.key,
      placement,
    ]),
  )
  return (
    before.length === after.size &&
    before.every((placement) => {
      const moved = after.get(placement.key)
      return (
        moved !== undefined &&
        Math.abs(moved.x - placement.x) < 1e-9 &&
        Math.abs(moved.y - placement.y) < 1e-9 &&
        Math.abs(moved.width - placement.width) < 1e-9 &&
        Math.abs(moved.height - placement.height) < 1e-9 &&
        moved.column === placement.column &&
        moved.row === placement.row
      )
    })
  )
}

/**
 * Each pane's zones a drag carrying `carried` must not aim at, as `layout`
 * and `room` stand when the press begins: where the drop would change
 * nothing (`movesNothing`, and a pane over itself), or the room refuses it.
 * Read once, as the targets are: any change to either ends the drag.
 */
export function refusedZones(
  layout: PaneLayout,
  carried: Carried,
  room: PaneRoom | undefined,
): ReadonlyMap<PaneKey, ReadonlySet<Zone>> {
  return new Map(
    placements(layout.columns).panes.map(({ key }) => [
      key,
      new Set(
        zones.filter((zone) => {
          const outcome = dropOutcome(layout, carried, key, zone, room)
          return outcome === null || movesNothing(layout, outcome)
        }),
      ),
    ]),
  )
}
