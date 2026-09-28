/**
 * A drag's life, as a value: what a press on a pane's header or a session's
 * row becomes, event by event. The table is ADR 238 › _Drag and drop_; each
 * row is a case below and at least one test in `drag.test.ts`. The page —
 * the copy, the preview, the command — is the adapter's
 * (`adapters/dom/drag.ts`), which sends every event here and draws what the
 * phase it gets back says.
 *
 * ```text
 *   idle ──press──▶ pressed ──move ≥ 4px──▶ carrying ──release on a zone──▶ dropping ──landed──▶ idle
 *                     │                        │ ──release elsewhere, Escape, lost──▶ cancelling (home) ──landed──▶ idle
 *                     │                        └──a change──▶ cancelling (at once) ──landed──▶ idle
 *                     └──release, Escape, lost, a change──▶ idle
 * ```
 *
 * Nothing is read again while carrying: any change to the room, the panes or
 * what the content region shows ends the drag instead (`changed`), so what
 * was read as the press began stays true for as long as it is used.
 */
import { aimAt, type Aim, type Carried, type PointerSample, type Targets } from "./drop"

/** How far the pointer moves, pressed, before a press becomes a drag. */
export const liftDistance = 4

export type DragPhase =
  | { readonly kind: "idle" }
  | {
      readonly kind: "pressed"
      readonly carried: Carried
      readonly pointerId: number
      readonly from: PointerSample
    }
  | {
      readonly kind: "carrying"
      readonly carried: Carried
      readonly pointerId: number
      /** Where the pointer has been lately, newest last: where it is and where it heads. */
      readonly path: readonly PointerSample[]
      /** What a release now would drop on (`aimAt`). */
      readonly aim: Aim | null
    }
  | { readonly kind: "dropping"; readonly carried: Carried; readonly aim: Aim }
  | {
      readonly kind: "cancelling"
      /**
       * `home`: nothing around it changed, so the copy flies back to where it
       * was lifted. `at-once`: the room, the panes or the view changed under
       * it, so where it came from may be gone — it goes where it is.
       */
      readonly how: "home" | "at-once"
    }

export type DragEvent =
  /** The primary button pressed on something that can be carried. */
  | {
      readonly kind: "press"
      readonly carried: Carried
      readonly pointerId: number
      readonly at: PointerSample
    }
  | {
      readonly kind: "move"
      readonly pointerId: number
      readonly at: PointerSample
      readonly targets: Targets | null
    }
  /** No move for a while: the zone is decided again, as at rest. */
  | { readonly kind: "still"; readonly t: number; readonly targets: Targets | null }
  | {
      readonly kind: "release"
      readonly pointerId: number
      readonly at: PointerSample
      readonly targets: Targets | null
      /** Whether dropping at `aim` would do anything (`dropOutcome`). */
      readonly offers: (aim: Aim) => boolean
    }
  | { readonly kind: "escape" }
  /** The page lost the pointer: `lostpointercapture`, `pointercancel`, the window's blur. */
  | { readonly kind: "lost" }
  /**
   * Something the drag read changed, or is about to: a key that is a command,
   * a resize, the panes, the content view or the side columns in the store,
   * the carried session gone, or the window going inert under Settings.
   */
  | { readonly kind: "changed" }
  /** The copy's last flight — onto its place, or home — ended. */
  | { readonly kind: "landed" }

export const idle: DragPhase = { kind: "idle" }

/** How much of the pointer's path is kept: twice what its heading reads. */
const kept = 250

const along = (path: readonly PointerSample[], at: PointerSample) => [
  ...path.filter((sample) => at.t - sample.t <= kept),
  at,
]

/** The phase `event` moves `phase` to; the same object where it changes nothing. */
export function stepDrag(phase: DragPhase, event: DragEvent): DragPhase {
  switch (phase.kind) {
    case "idle":
      return event.kind === "press"
        ? {
            kind: "pressed",
            carried: event.carried,
            pointerId: event.pointerId,
            from: event.at,
          }
        : phase
    case "pressed":
      switch (event.kind) {
        case "move": {
          if (event.pointerId !== phase.pointerId) return phase
          const { x, y } = phase.from
          if (Math.hypot(event.at.x - x, event.at.y - y) < liftDistance) return phase
          // The copy is shown in this frame; the zone waits for the next move or rest.
          return {
            kind: "carrying",
            carried: phase.carried,
            pointerId: phase.pointerId,
            path: [event.at],
            aim: null,
          }
        }
        case "release":
          return event.pointerId === phase.pointerId ? idle : phase
        case "escape":
        case "lost":
        case "changed":
          return idle
        default:
          return phase
      }
    case "carrying":
      switch (event.kind) {
        case "move": {
          if (event.pointerId !== phase.pointerId) return phase
          // A move to where it already is is no move: resting is not restarted.
          const last = phase.path.at(-1)
          if (last && last.x === event.at.x && last.y === event.at.y) return phase
          const path = along(phase.path, event.at)
          return {
            ...phase,
            path,
            aim: aimAt(path, event.at.t, phase.aim, event.targets),
          }
        }
        case "still": {
          const aim = aimAt(phase.path, event.t, phase.aim, event.targets)
          return aim?.target === phase.aim?.target && aim?.zone === phase.aim?.zone
            ? phase
            : { ...phase, aim }
        }
        case "release": {
          if (event.pointerId !== phase.pointerId) return phase
          // The zone under the release, as its path had it: a pause before
          // letting go has aged the heading out (`restAfter`).
          const last = phase.path.at(-1)
          const moved = !last || last.x !== event.at.x || last.y !== event.at.y
          const path = moved ? along(phase.path, event.at) : phase.path
          const aim = aimAt(path, event.at.t, phase.aim, event.targets)
          return aim && event.offers(aim)
            ? { kind: "dropping", carried: phase.carried, aim }
            : { kind: "cancelling", how: "home" }
        }
        case "escape":
        case "lost":
          return { kind: "cancelling", how: "home" }
        case "changed":
          return { kind: "cancelling", how: "at-once" }
        default:
          return phase
      }
    case "dropping":
    case "cancelling":
      return event.kind === "landed" ? idle : phase
  }
}

/** Keys that are only held with others: pressed alone, they command nothing. */
const modifiers = new Set(["Shift", "Control", "Alt", "Meta", "CapsLock", "Fn", "OS"])

/**
 * What a key pressed during a press or a drag is to it: Escape is Escape; any
 * other key but a lone modifier is a command about to change what the drag
 * read (⌘W, ⌘0, ⌘B, ⌘,, an arrow…), so it ends the drag first.
 */
export function keyToDrag(key: string): DragEvent | null {
  if (key === "Escape") return { kind: "escape" }
  return modifiers.has(key) ? null : { kind: "changed" }
}
