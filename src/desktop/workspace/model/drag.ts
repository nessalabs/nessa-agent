/**
 * A drag's life, as a value: what a press on a pane's header or a session's
 * row becomes, event by event. The table is ADR 238 › _Drag and drop_; each
 * row is a case below and at least one test in `drag.test.ts`. The page —
 * the copy, the preview, the command — is the adapter's
 * (`adapters/dom/drag.ts`), which sends every event here and draws what the
 * phase it gets back says. What the page made for the drag (`Made`) is held
 * in the phase, so whether a drag is live has one answer: the phase.
 *
 * ```text
 *   idle ──press──▶ pressed ──ready──▶ pressed, made ──move ≥ 4px──▶ carrying
 *   carrying ──release in the zone previewed──▶ dropping ──landed──▶ idle
 *   carrying ──release elsewhere, Escape, lost──▶ cancelling (home) ──landed──▶ idle
 *   carrying ──a change──▶ cancelling (at once) ──landed──▶ idle
 *   pressed ──release, Escape, lost, a change──▶ idle
 * ```
 *
 * Nothing is read again while carrying: any change to the room, the panes or
 * what the content region shows ends the drag instead (`changed`), so what
 * was read as the press began stays true for as long as it is used.
 */
import {
  aimAt,
  headingWindow,
  type Aim,
  type Carried,
  type PointerSample,
  type Targets,
} from "./drop"

/** How far the pointer moves, pressed, before a press becomes a drag. */
export const liftDistance = 4

/** `PointerEvent.buttons` with the primary button alone held: the one way to carry. */
export const primaryAlone = 1

export type DragPhase<Made = unknown> =
  | { readonly kind: "idle" }
  | {
      readonly kind: "pressed"
      readonly carried: Carried
      readonly pointerId: number
      readonly from: PointerSample
      /** What the page made for the drag once the press's frame painted; `null` until then. */
      readonly made: Made | null
    }
  | {
      readonly kind: "carrying"
      readonly carried: Carried
      readonly pointerId: number
      /** Where the pointer has been lately, newest last: where it is and where it heads. */
      readonly path: readonly PointerSample[]
      /** What a release now would drop on (`aimAt`). */
      readonly aim: Aim | null
      readonly made: Made
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

export type DragEvent<Made = unknown> =
  /** The primary button pressed on something that can be carried. */
  | {
      readonly kind: "press"
      readonly carried: Carried
      readonly pointerId: number
      readonly at: PointerSample
    }
  /** The page made what the drag needs, once the press's frame painted. */
  | { readonly kind: "ready"; readonly made: Made }
  | {
      readonly kind: "move"
      readonly pointerId: number
      /** `PointerEvent.buttons`: anything but the primary alone ends it (`primaryAlone`). */
      readonly buttons: number
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
      /** The aim the page has previewed an outcome for (`dropOutcome`), if any. */
      readonly previewed: Aim | null
    }
  | { readonly kind: "escape" }
  /**
   * The page lost the pointer — `lostpointercapture`, `pointercancel`, the
   * window's blur — or the pointer pressed another button.
   */
  | { readonly kind: "lost" }
  /**
   * Something the drag read changed, or is about to: a key that is a command,
   * a resize, the panes, the content view or the side columns in the store,
   * the carried session gone, the window going inert under Settings — or,
   * pressed, nothing to carry on the page.
   */
  | { readonly kind: "changed" }
  /** The copy's last flight — onto its place, or home — ended. */
  | { readonly kind: "landed" }

export const idle: DragPhase<never> = { kind: "idle" }

const sameAim = (a: Aim | null, b: Aim | null) =>
  a?.target === b?.target && a?.zone === b?.zone

/** How much of the pointer's path is kept: all its heading reads (`pointerVelocity`). */
const kept = headingWindow

const along = (path: readonly PointerSample[], at: PointerSample) => [
  ...path.filter((sample) => at.t - sample.t <= kept),
  at,
]

/** The phase `event` moves `phase` to; the same object where it changes nothing. */
export function stepDrag<Made>(
  phase: DragPhase<Made>,
  event: DragEvent<Made>,
): DragPhase<Made> {
  switch (phase.kind) {
    case "idle":
      return event.kind === "press"
        ? {
            kind: "pressed",
            carried: event.carried,
            pointerId: event.pointerId,
            from: event.at,
            made: null,
          }
        : phase
    case "pressed":
      switch (event.kind) {
        case "ready":
          return phase.made === null ? { ...phase, made: event.made } : phase
        case "move": {
          if (event.pointerId !== phase.pointerId) return phase
          if (event.buttons !== primaryAlone) return idle
          // Nothing made yet: it becomes a drag only once there is a copy to show.
          if (phase.made === null) return phase
          const { x, y } = phase.from
          if (Math.hypot(event.at.x - x, event.at.y - y) < liftDistance) return phase
          // The copy is shown in this frame; the zone waits for the next move or rest.
          return {
            kind: "carrying",
            carried: phase.carried,
            pointerId: phase.pointerId,
            path: [event.at],
            aim: null,
            made: phase.made,
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
          if (event.buttons !== primaryAlone) return { kind: "cancelling", how: "home" }
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
          return sameAim(aim, phase.aim) ? phase : { ...phase, aim }
        }
        case "release": {
          if (event.pointerId !== phase.pointerId) return phase
          // The zone under the release, as its path had it — a pause before
          // letting go has aged the heading out (`restAfter`) — and only if
          // it is the one the page previewed: nothing unseen is committed.
          const last = phase.path.at(-1)
          const moved = !last || last.x !== event.at.x || last.y !== event.at.y
          const path = moved ? along(phase.path, event.at) : phase.path
          const aim = aimAt(path, event.at.t, phase.aim, event.targets)
          return aim && sameAim(aim, event.previewed)
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
export function keyToDrag(
  key: string,
): { readonly kind: "escape" } | { readonly kind: "changed" } | null {
  if (key === "Escape") return { kind: "escape" }
  return modifiers.has(key) ? null : { kind: "changed" }
}
