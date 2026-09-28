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
  type Zone,
} from "./pane-layout"
import { arrange, type Arranged, type WorkspaceRoom } from "./pane-sizing"

/** How near a pane's edge, as a share of its size, a drop splits rather than lands in the middle. */
export const edgeShare = 0.26

/** What is carried: a pane by its header, or a session from a list. */
export type Carried =
  | { readonly kind: "pane"; readonly pane: PaneKey }
  | { readonly kind: "session"; readonly sessionId: string }

/**
 * The zone of a pane the pointer is in, from where it is across the pane (0
 * at its left or top, 1 at its right or bottom): the side it is nearest when
 * near enough, else the middle.
 */
export function zoneAt(x: number, y: number): Zone {
  const near: [Zone, number][] = [
    ["left", x],
    ["right", 1 - x],
    ["top", y],
    ["bottom", 1 - y],
  ]
  const [side, distance] = near.reduce((a, b) => (b[1] < a[1] ? b : a))
  return distance > edgeShare ? "center" : side
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
