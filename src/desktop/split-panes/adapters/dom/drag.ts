/**
 * Drag and drop between split panes, carried by the pointer rather than the
 * browser's own drag, so what is carried looks like the window it will be.
 * What a press becomes, event by event, is `model/drag.ts` (ADR 238 › _Drag
 * and drop_ has its table); this is the page's side of it — every event is
 * sent there, and each phase it answers with is drawn here:
 *
 * - **pressed**: nothing in the press's own frame; in a task once it has
 *   painted, the copy is made unseen — the pane itself, or for an item from
 *   outside the grid what the host makes of it (`copyOf`) — and what the drag
 *   needs of the page is read, once: the grid and its room, each pane's box
 *   and parts, what the host covers (`covered`). Until then a move does not
 *   lift it;
 * - **carrying**: the copy is shown under the pointer and glides so its
 *   centre comes under it, then stays there; the zone the pointer is in
 *   (`aimAt`) shows what dropping there would do — the panes take the rects
 *   the drop would give them and a placeholder marks the rect it takes, from
 *   the one outcome the drop commits (`dropOutcome` of the layout the source
 *   reads), and the copy takes the placeholder's shape about the pointer
 *   (`copyShape`);
 * - **dropping**: released while a zone is shown, the copy flies into the
 *   placeholder's rect. The source commits what is shown (`commitDrop`) on
 *   the next frame, in the room the press read, so that pointerup does not
 *   also lay the new arrangement out (`drag.test.tsx`). That frame commits
 *   only while the preview still holds — the panes and watched values the
 *   press read, a carried item still held, and no resize since the release —
 *   and not at all if the drag is gone before it (`drag.test.tsx`);
 * - **cancelling**: the copy flies home as the panes go back — or, when the
 *   room, the panes or the view changed under it, both go at once and the
 *   change plays as it would with no drag.
 *
 * Everything moves by transform, in animations the compositor runs; the
 * pointer is followed without rendering anything, and the outcome is asked
 * for only when the zone changes. What changes shape grows or shrinks into it by
 * a scale its content undoes step by step, so no text is ever drawn
 * stretched: a pane the preview resizes is cut to the shape it would take;
 * the copy is laid out once at it, as the zone changes. No frame of a drag
 * makes the page lay out early: after the press's frame nothing is read again — a change that would
 * make what was read wrong ends the drag instead. Where a preview has drawn a
 * pane is known from the preview's own motion, never read back. The
 * preview's motion is marked (`dragPreview`) so `FlipScope` measures through
 * it at the drop and lets it go before it measures where things landed. With
 * less motion, nothing glides: the copy follows the pointer, and the panes
 * and the copy take the drop's rects at once. The zone is said to assistive
 * technology as it changes.
 *
 * A host takes part by marking what can be carried: `data-drag-pane` (a
 * pane's key, on its header) or `data-drag-item` (an item's id, on a row
 * outside the grid), and by marking what each part of a pane keeps to as a
 * preview reshapes it (`data-split-keeps`, `PanePart`). The keyboard's own
 * ways — whatever moves a host gives a pane — are untouched.
 */
import { useEffect, type RefObject } from "react"
import type { SplitPanesSource } from "../../application/ports"
import {
  copyShape,
  idle,
  keyToDrag,
  ownsDragResources,
  sameAim,
  stepDrag,
  type DragEvent,
  type DragPhase,
  type Size,
} from "../../model/drag"
import {
  dropOutcome,
  refusedZones,
  restAfter,
  type Aim,
  type Carried,
  type DropOutcome,
  type PointerSample,
  type Targets,
} from "../../model/drop"
import type { PaneKey, PaneLayout, Zone } from "../../model/pane-layout"
import { paneLimits } from "../../model/pane-layout"
import { placements, type PanePlacement, type PaneRoom } from "../../model/pane-sizing"
import { durationToken, motionToken } from "../../../adapters/motion"
import { classes, gridOf, marks, reflectMark } from "./marks"

/** Marks the preview's own motion. */
const dragPreview = "split-panes-preview"

/** The preview's animations under each drag root, so they can be let go without a search. */
const previews = new WeakMap<Element, Set<Animation>>()

/** The panes a preview marked as resting in the window's corner, or leaving it, under each drag root. */
const cornered = new WeakMap<Element, Set<HTMLElement>>()

/** Whether `pane`'s header steps past the window's controls now. */
const steppedAside = (pane: HTMLElement) => {
  const corner = pane.getAttribute(marks.dragCorner)
  return corner === "yes" || (corner !== "no" && pane.hasAttribute(marks.corner))
}

/**
 * Steps `pane`'s header past the window's controls where a drop would rest
 * it in the corner, and back where it would leave it (`data-drag-corner`,
 * `panes.css`) — or, with `null`, as its placement does.
 */
function markCorner(root: Element, pane: HTMLElement, corner: boolean | null) {
  const held = cornered.get(root) ?? new Set<HTMLElement>()
  cornered.set(root, held)
  const differs = corner !== null && corner !== pane.hasAttribute(marks.corner)
  if (differs) held.add(pane)
  else held.delete(pane)
  const value = differs ? (corner ? "yes" : "no") : null
  if (pane.getAttribute(marks.dragCorner) === value) return
  if (value) pane.setAttribute(marks.dragCorner, value)
  else pane.removeAttribute(marks.dragCorner)
}

function track(root: Element, animation: Animation): Animation {
  const held = previews.get(root) ?? new Set<Animation>()
  previews.set(root, held)
  held.add(animation)
  void animation.finished.catch(() => undefined)
  return animation
}

/** A pane drawn away from where it is laid out, about its centre: moved, scaled, faded. */
interface Drawn {
  readonly dx: number
  readonly dy: number
  readonly sx: number
  readonly sy: number
  readonly opacity: number
}

const transformOf = ({ dx, dy, sx, sy }: Drawn) =>
  `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`

/**
 * How many steps a change of shape is drawn in. A box scaled on one curve and
 * its content scaled back on the same curve only cancel where the two are
 * given together, so the content's undoing is given at each step: between
 * two, it is out by less than a percent.
 */
const shapeSteps = 12

const lerp = (a: number, b: number, progress: number) => a + (b - a) * progress

const sameSize = (a: Size, b: Size) =>
  Math.abs(a.width - b.width) < 0.5 && Math.abs(a.height - b.height) < 0.5

/**
 * Starts a box's change of shape and its content's undoing of it at one
 * time on the document's clock, so neither is ever drawn a frame ahead of the
 * other — which would draw the content stretched for that frame. Left to
 * start when each is ready, an engine may start them a frame apart.
 */
/** Canceling a pending animation rejects `ready` with `AbortError`. That is letting go. */
function readyWasCancelled(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "name" in error &&
    (error as { name?: unknown }).name === "AbortError"
  )
}

function together(animations: readonly Animation[]) {
  const now = document.timeline?.currentTime
  if (now === null || now === undefined) return
  for (const animation of animations) animation.startTime = now
  // WebKit can replace that start when `ready` fulfills, so a pair given one
  // instant is drawn a frame apart (#254). Put the same instant back on any
  // that are still running. An animation with no `ready` (the jsdom stand-in)
  // keeps the start just set. Letting the preview go cancels a pending
  // animation and rejects `ready`; that rejection is expected. Any other
  // rejection is raised again so it is not swallowed.
  const pending = animations.filter(
    (animation) => typeof animation.ready?.then === "function",
  )
  if (pending.length === 0) return
  void Promise.all(
    pending.map((animation) =>
      animation.ready.then(
        () => {
          // A preview already let go is `idle`. Setting its start again would
          // play it. Only a pair still running needs the shared instant.
          if (animation.playState !== "running") return
          if (animation.startTime !== now) animation.startTime = now
        },
        (error: unknown) => {
          if (readyWasCancelled(error)) return
          throw error
        },
      ),
    ),
  ).catch((error: unknown) => {
    const reported = error instanceof Error ? error : new Error(String(error))
    // `reportError` is the page's uncaught-exception path, so a failure here
    // is recorded with other page errors. Cancellation never reaches this.
    if (typeof reportError === "function") reportError(reported)
    else console.error(reported)
  })
}

/** How far along its clock an animation is drawn, eased; 1 once it has ended or with none. */
const progressOf = (animation: Animation | null) => {
  const progress = animation?.effect?.getComputedTiming().progress
  return progress === null || progress === undefined ? 1 : progress
}

/**
 * Lets go of a drag's preview under `root`: what `FlipScope` does before it
 * measures where a drop landed, so a drop that lands where it previewed has
 * nowhere to fly. Nothing to do, and no search, when no drag previewed.
 */
export function letGoOfDragPreview(root: Element): void {
  cornered.get(root)?.forEach((pane) => markCorner(root, pane, null))
  cornered.delete(root)
  const held = previews.get(root)
  if (!held) return
  held.forEach((animation) => animation.cancel())
  previews.delete(root)
}

interface Box {
  readonly left: number
  readonly top: number
  readonly width: number
  readonly height: number
}

const boxOf = (rect: DOMRect | Box): Box => ({
  left: rect.left,
  top: rect.top,
  width: rect.width,
  height: rect.height,
})

/**
 * Where something the host covers the grid with is once it has settled. It
 * rests at no transform, so one still sliding in — a column revealed from
 * the window's edge just before the press — is taken at the rect it is sliding to,
 * its box less the translation it is drawn at now, never the part of the way
 * it has come.
 */
function settledBox(column: Element): Box {
  const rect = column.getBoundingClientRect()
  const transform = getComputedStyle(column).transform
  const values = /^matrix(3d)?\(([^)]*)\)$/.exec(transform)
  if (!values) return boxOf(rect)
  const numbers = (values[2] ?? "").split(",").map(Number)
  const [dx, dy] = values[1] ? [numbers[12], numbers[13]] : [numbers[4], numbers[5]]
  return {
    left: rect.left - (dx ?? 0),
    top: rect.top - (dy ?? 0),
    width: rect.width,
    height: rect.height,
  }
}

/** Where a placement is drawn in a grid laid out at `grid`, as the stylesheet draws it. */
function drawn(placement: PanePlacement, grid: Box): Box {
  const g = paneLimits.gutter
  const width = grid.width - (placement.columns - 1) * g
  const height = grid.height - (placement.rows - 1) * g
  return {
    left: grid.left + width * placement.x + placement.column * g,
    top: grid.top + height * placement.y + placement.row * g,
    width: width * placement.width,
    height: height * placement.height,
  }
}

/** Every pane's box in `layout`, laid out in `grid`. */
function boxes(layout: PaneLayout, grid: Box): Map<PaneKey, Box> {
  return new Map(
    placements(layout.columns).panes.map((placement) => [
      placement.key,
      drawn(placement, grid),
    ]),
  )
}

/** How an element laid out at `from` is drawn at `to`, about its centre. */
function between(from: Box, to: Box, opacity = 1): Drawn {
  return {
    dx: to.left + to.width / 2 - (from.left + from.width / 2),
    dy: to.top + to.height / 2 - (from.top + from.height / 2),
    sx: to.width / from.width,
    sy: to.height / from.height,
    opacity,
  }
}

/** The grid a drop would leave, the host's spare room added when it takes it. */
const landingGrid = (grid: Box, spare: number): Box => ({
  ...grid,
  left: grid.left - spare,
  width: grid.width + spare,
})

/** A preview rect held inside the grid it is drawn in: nothing previews off the grid. */
const inside = (box: Box, grid: Box): Box => {
  const left = Math.max(box.left, grid.left)
  const top = Math.max(box.top, grid.top)
  return {
    left,
    top,
    width: Math.min(box.left + box.width, grid.left + grid.width) - left,
    height: Math.min(box.top + box.height, grid.top + grid.height) - top,
  }
}

/** What the zone does, as it is said. */
export function saying(outcome: DropOutcome, zone: Zone, title: string): string {
  const where = zone === "top" ? "above" : zone === "bottom" ? "below" : `${zone} of`
  switch (outcome.does) {
    case "swap":
      return `Swap with ${title}`
    case "move":
      return `Move ${where} ${title}`
    case "replace":
      return `Open in place of ${title}`
    case "split":
      return `Split ${where} ${title}`
    case "go-to":
      return `Go to ${title}`
  }
}

/**
 * What the page made for one drag, once the press's frame painted, and what
 * it draws while carrying. Held in the drag's phase (`DragPhase<Made>`), so
 * the phase is the one answer to whether a drag is live; its drawing state
 * is the page's, written as the page is.
 */
interface Made {
  /** Everything carried is drawn in it: a layer that begins below the titlebar row. */
  readonly layer: HTMLElement
  /** Moved to the pointer, one to one; holds the copy. */
  readonly carrier: HTMLElement
  /** Where the copy was grabbed, gliding to where its centre is under the pointer. */
  readonly glider: HTMLElement
  /** The copy, drawn about its centre: it takes the shape of where it would land. */
  readonly ghost: HTMLElement
  /** The copy's content, counter-scaled as its box changes shape. */
  readonly inner: HTMLElement
  /** What was pressed: marked while carried. */
  readonly pressed: HTMLElement
  /** Over the page while carrying: the grabbing hand, and nothing under it selected. */
  readonly shield: HTMLElement
  /** Where the copy started, and flies back to. */
  readonly home: Box
  /** Where the pointer holds the copy as it is lifted, from its top left. */
  readonly grab: { x: number; y: number }
  /** The carried pane's own size: what the copy is drawn at with no zone shown. */
  readonly size: Size
  readonly grid: Box
  /** The panes' room as the press found it: what the preview and the drop are held to. */
  readonly room: PaneRoom | undefined
  /** What the drag may aim at; `null` while no pane can be seen. */
  readonly targets: Targets | null
  /** Each pane's parts and where each sits in it: what a preview counter-scales. */
  readonly parts: ReadonlyMap<HTMLElement, readonly PanePart[]>
  readonly motion: { duration: number; easing: string }
  /** What is drawn now, written as the drag goes. */
  readonly drawing: {
    pointer: { x: number; y: number }
    /** The copy's glide, from where it was grabbed to its centre under the pointer. */
    glide: Animation | null
    /** The drop's final fade, retained until its resources are released. */
    fade: Animation | null
    /** The size the copy is laid out at: the last shape it was asked to take. */
    laid: Size
    /** The copy's change of shape, from the size it was drawn at to `laid`. */
    shape: { from: Size; to: Size; motion: Animation | null; counter: Animation | null }
    /** The frame asked for to show the zone; 0 when none is. */
    frame: number
    /** The timer that says the pointer has been still long enough to be at rest; 0 when none. */
    still: number
    /** The aim the page shows, and what it does: what a release may commit. */
    shown: Aim | null
    outcome: DropOutcome | null
    outline: HTMLElement | null
    /** Where the drop would land, if it would. */
    landing: Box | null
  }
}

/**
 * A part of a pane a preview draws at its own size, and what of the pane's
 * would-be box it keeps to, as a pane that shape would lay it out — as the
 * host marks it with `data-split-keeps`: `top-left` (a header, which steps
 * past the window's controls in the corner), `foot` (what docks at the
 * foot, a composer), `middle` (what a pane centres, an empty pane's
 * content); unmarked, the top. Across, the top keeps to the middle where
 * the pane grows — a pane centres its column of content — and to the left where it shrinks, so what is cut is the end of its lines,
 * never their start.
 */
interface PanePart {
  readonly element: HTMLElement
  /** Where it sits in its pane, from the pane's top left, in pixels. */
  readonly left: number
  readonly top: number
  readonly keeps: "top-left" | "top" | "foot" | "middle"
}

/** What a part may be marked as keeping to (`data-split-keeps`); unmarked is the top. */
const keepings = ["top-left", "foot", "middle"] as const

const isMark = (value: string | undefined): value is (typeof keepings)[number] =>
  keepings.some((keeping) => keeping === value)

/** What a part keeps to, as the host marked it; unmarked, or marked with anything else, the top. */
function keepsOf(element: HTMLElement): PanePart["keeps"] {
  const marked = element.getAttribute(marks.keeps) ?? undefined
  return isMark(marked) ? marked : "top"
}

/**
 * A pane's parts, read as the press begins: its children, and the children
 * of any the host marks `data-split-through` (a wrapper to look inside), so
 * what scrolls and what docks at the foot are parts apart.
 */
function partsOf(pane: HTMLElement): PanePart[] {
  // Laid out, not drawn: offsets, summed up to the pane, ignore any transform.
  const offset = (element: HTMLElement) => {
    let left = 0
    let top = 0
    for (
      let at: HTMLElement | null = element;
      at && at !== pane;
      at = at.offsetParent as HTMLElement | null
    ) {
      left += at.offsetLeft
      top += at.offsetTop
    }
    return { left, top }
  }
  const part = (element: HTMLElement): PanePart => {
    const { left, top } = offset(element)
    return {
      element,
      left,
      top,
      keeps: keepsOf(element),
    }
  }
  const within = (element: Element): HTMLElement[] =>
    Array.from(element.children, (child) =>
      child.hasAttribute(marks.through) ? within(child) : [child as HTMLElement],
    ).flat()
  return within(pane).map(part)
}

/** What the host adds to a drag of its own: what only it knows of its page. */
export interface SplitPanesDragOptions {
  /**
   * The copy carried for an item pressed outside the grid (`data-drag-item`),
   * built detached — never put on the page, and nothing on the page read
   * after the press. `pressed` is what was pressed; `picture` copies an
   * element as the drag copies a pane (ids, labels, and the marks that
   * would make it something to carry, aim at or mid-drag stripped: see
   * `alwaysStripped`); `focusedPane` is the focused pane's element, if drawn.
   */
  readonly copyOf: (
    item: string,
    context: {
      readonly pressed: HTMLElement
      readonly picture: (element: Element) => HTMLElement
      readonly focusedPane: HTMLElement | null
    },
  ) => HTMLElement
  /**
   * What the host draws over or beside the grid under `root` as the press
   * begins, never a target whatever is under it (side columns, docked or
   * revealed from the window's edge).
   */
  readonly covered: (root: HTMLElement) => readonly Element[]
  /** Attributes of the host's a copy leaves out, beside the module's own. */
  readonly stripped?: readonly string[]
  /**
   * Subtrees a copy does not picture, matched on the pane's own children.
   * The conversation is one: it is not painted, and cloning it is the press's
   * long frame (`split-panes-drag.test.tsx`).
   */
  readonly dropped?: readonly string[]
  /**
   * Parts a preview does not scale back. The conversation is one: it is not
   * painted while the preview moves, so a transform of its own would be a
   * layer that is never seen (`split-panes-drag.test.tsx`).
   */
  readonly unscaled?: readonly string[]
}

/**
 * What a copy leaves out of what it pictures: identity and focus, and what
 * would make it something to carry, aim at or treat as mid-drag. What says
 * how the pane is laid out — in the corner (`marks.corner`,
 * `marks.dragCorner`) — stays, so the copy's header starts where the pane's
 * does.
 */
const alwaysStripped = [
  "id",
  "tabindex",
  "aria-label",
  "aria-labelledby",
  "aria-describedby",
  "data-pane-key",
  "data-flip",
  "data-flip-id",
  marks.dragPane,
  marks.dragItem,
  marks.carrying,
  marks.lifted,
  marks.waiting,
]

/**
 * Drag and drop under `root`, which spans the host — items are picked up
 * outside the grid and dropped on it. `source` and `options` are built once
 * by the host: a new one ends any drag and starts again.
 */
export function useSplitPanesDrag(
  root: RefObject<HTMLElement | null>,
  source: SplitPanesSource,
  options: SplitPanesDragOptions,
): void {
  useEffect(() => {
    const scope = root.current
    if (!scope) return

    const announcer = document.createElement("div")
    announcer.className = "split-panes-announcer"
    announcer.setAttribute("role", "status")
    announcer.setAttribute("aria-live", "polite")
    scope.append(announcer)

    /** The phase of the one drag (`model/drag.ts`), holding what the page made for it. */
    let phase: DragPhase<Made> = idle
    /** What is being carried now, if anything. */
    const carried = () => (phase.kind === "carrying" ? phase.made : null)

    /** The press's frame, then the task after it, that make what a drag needs. */
    let waiting: { frame: number; timer: number } | null = null
    /** Bumped whenever glass blur is held or released, so a late release cannot drop a new drag's hold. */
    let glass = 0
    /**
     * The next body waits. That frame is the glass coming back, so it does not
     * also paint a transcript into the sidebar's blur (`drag.test.tsx`).
     */
    let revealWaits = false
    const holdGlass = (on: boolean) => {
      const generation = ++glass
      if (on) {
        revealWaits = false
        reflectMark(scope, marks.pressing, true)
        return
      }
      // The next frame brings the first body back. Glass comes back the frame
      // after that, and that frame brings no body with it (`drag.test.tsx`).
      requestAnimationFrame(() => {
        if (generation !== glass) return
        if (scope.querySelector(`[${marks.settling}]`)) revealWaits = true
        requestAnimationFrame(() => {
          if (generation === glass) reflectMark(scope, marks.pressing, false)
        })
      })
    }
    /** Drops the pending commit's frame and its listeners, if a drop is waiting on one. */
    let releaseDrop: (() => void) | null = null

    /** One pane a frame, so letting the preview go does not lay every transcript out (`drag.test.tsx`). */
    const revealSettling = () => {
      if (revealWaits) {
        revealWaits = false
        if (scope.querySelector(`[${marks.settling}]`))
          requestAnimationFrame(revealSettling)
        return
      }
      const pane = scope.querySelector<HTMLElement>(`[${marks.settling}]`)
      pane?.removeAttribute(marks.settling)
      if (scope.querySelector(`[${marks.settling}]`))
        requestAnimationFrame(revealSettling)
    }

    /**
     * The preview's mark, on the grid only. A copy on the workspace or the
     * document would restyle the sidebar on the same frame (`drag.test.tsx`).
     */
    const reflowMark = (on: boolean) => {
      const grid = gridOf(scope)
      if (!grid || grid.hasAttribute(marks.reflow) === on) return
      grid.toggleAttribute(marks.reflow, on)
    }

    /**
     * Drops the preview's mark. Bodies are already waiting, or start waiting
     * now, and return one a frame: clearing the mark alone lays every
     * transcript out on that frame (`drag.test.tsx`).
     */
    const releaseReflow = () => {
      const grid = gridOf(scope)
      if (!grid?.hasAttribute(marks.reflow)) return
      const waiting = scope.querySelector(`[${marks.settling}]`) !== null
      if (!waiting) {
        scope.querySelectorAll<HTMLElement>("[data-pane-key]").forEach((pane) => {
          pane.setAttribute(marks.settling, "")
        })
        requestAnimationFrame(revealSettling)
      }
      reflowMark(false)
    }

    /**
     * What the press found: the panes' arrangement — not which has focus:
     * pressing a pane focuses it — and what the host watches. A change to any
     * of it ends the press or the drag.
     */
    let seen: { columns: unknown; watched: readonly unknown[] } = {
      columns: null,
      watched: [],
    }

    const layoutNow = () => source.layout()
    const paneElement = (key: PaneKey) =>
      scope.querySelector<HTMLElement>(`[data-pane-key="${key}"]`)

    // ——— The preview: panes where the drop would put them ———

    // Each pane's preview, as it was asked for: where it is drawn is where
    // this says, part way on its clock — never read back from the page.
    const previewed = new Map<
      HTMLElement,
      {
        from: Box
        to: Box
        opacity: { from: number; to: number }
        motion: Animation
        parts: Animation[]
      }
    >()
    /** Where a pane laid out at `real` is drawn now, and how faded. */
    const drawnNow = (pane: HTMLElement, real: Box) => {
      const held = previewed.get(pane)
      if (!held) return { box: real, opacity: 1 }
      const progress = progressOf(held.motion)
      // Size is the target from the first frame. The place is what glides
      // (`reflow`), so a later zone starts from that size, not a size in between.
      return {
        box: {
          left: lerp(held.from.left, held.to.left, progress),
          top: lerp(held.from.top, held.to.top, progress),
          width: held.to.width,
          height: held.to.height,
        },
        opacity: lerp(held.opacity.from, held.opacity.to, progress),
      }
    }
    const letGo = (pane: HTMLElement) => {
      const held = previewed.get(pane)
      if (!held) return
      previewed.delete(pane)
      markCorner(scope, pane, null)
      if (pane.style.clipPath) pane.style.clipPath = ""
      for (const animation of [held.motion, ...held.parts]) {
        const target = animation.effect?.target
        if (target instanceof HTMLElement && target.style.clipPath)
          target.style.clipPath = ""
        animation.cancel()
        previews.get(scope)?.delete(animation)
      }
    }

    /**
     * Draws a pane laid out at `real` at `to` — or where it is laid out — from
     * where it is drawn now. Its box takes the rect by transform, and what it
     * holds is scaled back at every step (`shapeSteps`), so the pane is drawn
     * the shape it would take — its content cut to it, never stretched — and
     * nothing is laid out again: a pane laid out at another size restyles and
     * lays out all it holds (it is a size container), more than a frame's
     * budget at every zone change (ADR 238 › _Drag and drop_). It only writes.
     */
    const reflow = (
      made: Made,
      pane: HTMLElement,
      real: Box,
      to: Box | null,
      fade = false,
      corner = false,
    ) => {
      const from = drawnNow(pane, real)
      const target = to ?? real
      const opacity = { from: from.opacity, to: to && fade ? 0.2 : 1 }
      const resting = to ? corner : pane.hasAttribute(marks.corner)
      // A header stepped past the window's controls stays so until its pane
      // has moved out from under them.
      const leaving = steppedAside(pane) && !resting
      letGo(pane)
      markCorner(scope, pane, resting || leaving)
      const parts = made.parts.get(pane) ?? []
      // What keeps to the top ends where a composer at the foot begins: cut
      // by as much as the pane is shorter, it never runs under the composer.
      // The cut is set once. Animating it would lay the transcript out on
      // every frame of the glide (`drag.test.tsx`).
      const docked = parts.some(({ keeps }) => keeps === "foot")
      const box: Keyframe[] = []
      const counter: Keyframe[] = []
      for (let step = 0; step <= shapeSteps; step++) {
        const progress = step / shapeSteps
        // The size is the target on every frame, including the first, and the
        // place glides. The content's scale cancels that size
        // (`split-panes-drag.test.tsx`).
        const placed = {
          left: lerp(from.box.left, target.left, progress),
          top: lerp(from.box.top, target.top, progress),
          width: target.width,
          height: target.height,
        }
        const drawing = between(real, placed, lerp(opacity.from, opacity.to, progress))
        // Opacity stays off the keyframes while the pane is opaque. An opacity
        // track, even one that holds at 1, makes the compositor blend the pane
        // with the blur on every frame of the glide.
        const frame: Keyframe = { transform: transformOf(drawing) }
        if (opacity.from !== 1 || opacity.to !== 1) frame.opacity = drawing.opacity
        box.push(frame)
        counter.push({ transform: `scale(${1 / drawing.sx}, ${1 / drawing.sy})` })
      }
      pane.style.transformOrigin = "50% 50%"
      const motion = track(
        scope,
        pane.animate(box, { ...made.motion, fill: "forwards", id: dragPreview }),
      )
      // The content keeps its size, as in a flight (`flip.tsx`), scaled back
      // about the point of the pane each part keeps to (`PanePart`).
      const shrinks = target.width < real.width
      const clip =
        docked && target.height < real.height - 0.5
          ? `inset(0 0 ${real.height - target.height}px 0)`
          : ""
      const unscaled = options.unscaled ?? []
      const undone = parts.flatMap(({ element, left, top, keeps }) => {
        if (unscaled.some((selector) => element.matches(selector))) return []
        const across =
          keeps === "top-left" || (keeps !== "middle" && shrinks) ? 0 : real.width / 2
        const down =
          keeps === "foot" ? real.height : keeps === "middle" ? real.height / 2 : 0
        element.style.transformOrigin = `${across - left}px ${down - top}px`
        element.style.clipPath = keeps === "top" ? clip : ""
        return track(
          scope,
          element.animate(counter, {
            ...made.motion,
            fill: "forwards",
            id: dragPreview,
          }),
        )
      })
      together([motion, ...undone])
      previewed.set(pane, { from: from.box, to: target, opacity, motion, parts: undone })
      if (leaving)
        void motion.finished
          .then(() => {
            if (previewed.get(pane)?.motion === motion) markCorner(scope, pane, resting)
          })
          .catch(() => undefined)
    }

    /**
     * The calm placeholder where the drop would land: a soft fill and a
     * hairline in the theme's edge light, at exactly the rect the pane will
     * take. It glides between zones by transform. Its width and height are
     * set at once — transitioning them would lay the page out every frame.
     */
    const placeholder = (made: Made, box: Box | null) => {
      const { drawing } = made
      drawing.landing = box
      if (!box) {
        drawing.outline?.remove()
        drawing.outline = null
        return
      }
      const fresh = !drawing.outline
      const element = drawing.outline ?? document.createElement("div")
      element.className = classes.placeholder
      Object.assign(element.style, {
        transform: `translate(${box.left}px, ${box.top}px)`,
        width: `${box.width}px`,
        height: `${box.height}px`,
      })
      if (fresh) scope.append(element)
      drawing.outline = element
    }

    /** Shows what dropping at `aim` would do, or — with nothing offered — nothing. It only writes. */
    const preview = (made: Made, outcome: DropOutcome | null, aim: Aim | null) => {
      const layout = layoutNow()
      if (!layout) return
      const { grid, room } = made
      const real = boxes(layout, grid)
      const spare = outcome?.takesSpare ? (room?.spare ?? 0) : 0
      const landing = outcome ? boxes(outcome.layout, landingGrid(grid, spare)) : null
      const corners = new Set(
        outcome
          ? placements(outcome.layout.columns).panes.flatMap((placement) =>
              placement.corner ? [placement.key] : [],
            )
          : [],
      )
      if (scope.hasAttribute(marks.takesSpare) !== Boolean(outcome?.takesSpare))
        scope.toggleAttribute(marks.takesSpare, Boolean(outcome?.takesSpare))
      if (outcome && landing && !gridOf(scope)?.hasAttribute(marks.reflow))
        reflowMark(true)
      // With less motion too: the panes take their rects at once
      // (`--desktop-base` is 0ms), or a swap's placeholder, under the copy,
      // would be all that showed.
      for (const [key, box] of real) {
        const pane = paneElement(key)
        if (!pane) continue
        const to = landing?.get(key) ?? null
        const replaced = outcome?.does === "replace" && key === outcome.lands
        reflow(
          made,
          pane,
          box,
          outcome && to ? inside(to, landingGrid(grid, spare)) : null,
          replaced,
          corners.has(key),
        )
      }
      const lands = outcome && landing ? (landing.get(outcome.lands) ?? null) : null
      placeholder(made, lands && inside(lands, landingGrid(grid, spare)))
      announcer.textContent =
        outcome && aim
          ? saying(
              outcome,
              aim.zone,
              paneElement(aim.target)?.getAttribute("aria-label") ?? "the pane",
            )
          : ""
    }

    /**
     * Brings what the page shows up to the phase's aim: the outcome asked for
     * — of the layout the source holds, in the room the press read, the one
     * the drop commits — only when it changed.
     */
    const show = () => {
      if (phase.kind !== "carrying") return
      const { aim, carried: what, made } = phase
      const { drawing } = made
      drawing.frame = 0
      if (sameAim(aim, drawing.shown)) return
      drawing.shown = aim
      const layout = layoutNow()
      // A pane over itself, or a zone the room refuses: nothing is offered.
      const outcome =
        aim && layout ? dropOutcome(layout, what, aim.target, aim.zone, made.room) : null
      if (outcome === drawing.outcome) return
      drawing.outcome = outcome
      preview(made, outcome, aim)
      // The copy takes the shape of the slot it would land in, or — with
      // nothing offered — its own, its centre still on the pointer.
      const shape = copyShape(made.size, drawing.landing)
      if (!sameSize(shape, drawing.shape.to)) reshape(made, shape)
    }

    // ——— The copy ———

    /** Where the copy's centre is drawn from the pointer: where it was grabbed, gliding to nought. */
    const glideNow = ({ grab, size, drawing }: Made) => {
      const rest = 1 - progressOf(drawing.glide)
      return {
        x: (size.width / 2 - grab.x) * rest,
        y: (size.height / 2 - grab.y) * rest,
      }
    }

    /**
     * Draws the copy at `size`, its centre moved by `by` from the pointer, in
     * `made.motion`'s time. Its width is laid out at `size` — once, as the
     * shape is asked for — and drawn at that width the whole way, scale 1,
     * so the glide only moves it (`split-panes-drag.test.tsx`). Its height
     * is `size`'s, so the copy's centre stays on the pointer
     * (`split-panes-drag.test.tsx`). Only a shape it is not laid out at
     * already lays anything out.
     */
    const reshape = (
      made: Made,
      size: Size,
      by: { x: number; y: number } = { x: 0, y: 0 },
      opacity?: { from: number; to: number },
    ): Animation => {
      const { ghost, inner, drawing } = made
      drawing.shape.motion?.cancel()
      drawing.shape.counter?.cancel()
      if (!sameSize(size, drawing.laid)) {
        ghost.style.width = `${size.width}px`
        ghost.style.height = `${size.height}px`
        drawing.laid = size
      }
      const box: Keyframe[] = []
      const counter: Keyframe[] = []
      // Laid out at the target size already. Scale stays 1, so the glide only
      // moves the copy; a changing scale would raster it on every frame
      // (`split-panes-drag.test.tsx`).
      for (let step = 0; step <= shapeSteps; step++) {
        const progress = step / shapeSteps
        box.push({
          transform: `translate(${by.x * progress - size.width / 2}px, ${by.y * progress - size.height / 2}px) scale(1, 1)`,
          ...(opacity ? { opacity: lerp(opacity.from, opacity.to, progress) } : {}),
        })
        counter.push({ transform: "scale(1, 1)" })
      }
      const options: KeyframeAnimationOptions = { ...made.motion, fill: "forwards" }
      const motion = ghost.animate(box, options)
      const undone = inner.animate(counter, options)
      together([motion, undone])
      drawing.shape = { from: drawing.shape.to, to: size, motion, counter: undone }
      return motion
    }

    /**
     * The copy flies into `box` — onto its place, or home — from where it is
     * drawn now: its glide is held where it has got to, and it takes the
     * box's shape (`reshape`) as its centre travels to the box's.
     */
    const flyTo = (made: Made, box: Box, fade: boolean) => {
      const { glider, drawing } = made
      const glide = glideNow(made)
      drawing.glide?.cancel()
      drawing.glide = null
      glider.style.transform = `translate(${glide.x}px, ${glide.y}px)`
      const by = {
        x: box.left + box.width / 2 - drawing.pointer.x - glide.x,
        y: box.top + box.height / 2 - drawing.pointer.y - glide.y,
      }
      return reshape(made, box, by, { from: 1, to: fade ? 0 : 1 })
    }

    const ghostFor = (
      carried: Carried,
      pressed: HTMLElement,
      size: { width: number; height: number },
    ) => {
      const ghost = document.createElement("div")
      ghost.className = classes.ghost
      ghost.setAttribute("aria-hidden", "true")
      ghost.inert = true
      Object.assign(ghost.style, {
        width: `${size.width}px`,
        height: `${size.height}px`,
        transform: `translate(${-size.width / 2}px, ${-size.height / 2}px)`,
      })
      const inner = document.createElement("div")
      inner.className = "split-panes-ghost-inner"
      ghost.append(inner)
      const stripped = [...alwaysStripped, ...(options.stripped ?? [])]
      const picture = (
        element: Element,
        prune?: (from: Element, copy: Element) => void,
      ): HTMLElement => {
        // Shallow, then each child whole — except a dropped subtree, which is
        // never built (`SplitPanesDragOptions.dropped`).
        const clone = element.cloneNode(false) as HTMLElement
        const omit = options.dropped ?? []
        for (const child of element.childNodes) {
          if (
            child instanceof Element &&
            omit.some((selector) => child.matches(selector))
          )
            continue
          clone.append(child.cloneNode(true))
        }
        prune?.(element, clone)
        for (const node of [clone, ...clone.querySelectorAll<HTMLElement>("*")]) {
          for (const name of stripped) node.removeAttribute(name)
        }
        return clone
      }
      /**
       * The conversation as it shows, and no more: the copy is a picture of
       * one screen of what the host marks `data-split-scroll` (its content
       * the scroller's first child), so what is scrolled out of view above
       * stands in as one spacer of its height and what is below is left out,
       * and the copy is moved up by where it was scrolled to — a transform,
       * never a scroll, which would lay the copy out at once.
       */
      const onScreenOnly = (from: Element, to: Element) => {
        const scroller = from.querySelector<HTMLElement>(`[${marks.scroll}]`)
        const content = scroller?.firstElementChild
        const copied = to.querySelector<HTMLElement>(`[${marks.scroll}] > :first-child`)
        if (!scroller || !content || !copied) return
        const view = scroller.getBoundingClientRect()
        const parts = Array.from(content.children, (child) =>
          child.getBoundingClientRect(),
        )
        const copies = Array.from(copied.children)
        const firstShown = parts.findIndex((part) => part.bottom > view.top)
        if (firstShown >= 0) {
          parts.forEach((part, index) => {
            if (index < firstShown || part.top >= view.bottom) copies[index]?.remove()
          })
          if (firstShown > 0) {
            const spacer = document.createElement("div")
            spacer.style.height = `${parts[firstShown].top - parts[0].top}px`
            copied.prepend(spacer)
          }
        }
        if (scroller.scrollTop > 0)
          copied.style.transform = `translateY(${-scroller.scrollTop}px)`
      }
      if (carried.kind === "pane") {
        const pane = paneElement(carried.pane)
        if (pane) {
          const copy = picture(pane, onScreenOnly)
          copy.removeAttribute("style")
          copy.classList.add("split-panes-ghost-pane")
          const typed = pane.querySelectorAll("textarea")
          copy.querySelectorAll("textarea").forEach((field, index) => {
            field.value = typed[index]?.value ?? ""
          })
          inner.append(copy)
          return { ghost, inner }
        }
      }
      // An item with no pane yet: what the host makes of it.
      const layout = layoutNow()
      const copy = options.copyOf(carried.kind === "item" ? carried.item : "", {
        pressed,
        picture,
        focusedPane: layout ? paneElement(layout.focused) : null,
      })
      copy.classList.add("split-panes-ghost-pane")
      inner.append(copy)
      return { ghost, inner }
    }

    /**
     * Everything a drag will need, read and made in a task after the press's
     * frame has painted — the copy put on the page unseen. The page is read
     * here and nowhere after: the grid and the room the source measures,
     * each pane's box and parts, and what the host covers.
     * A page something wrote to since that frame is laid out by these reads,
     * once (ADR 238 › _Drag and drop_ has the measured cost). A press that
     * never becomes a drag takes it all away again.
     */
    const prepare = (
      carried: Carried,
      pressed: HTMLElement,
      x: number,
      y: number,
    ): Made | null => {
      const layout = layoutNow()
      const panes = gridOf(scope)
      const room = source.measure()
      if (!layout || !panes || !room) return null
      const rect = panes.getBoundingClientRect()
      const grid: Box = {
        left: rect.left,
        top: rect.top,
        width: room.width,
        height: room.height,
      }
      const from =
        carried.kind === "pane"
          ? paneElement(carried.pane)?.getBoundingClientRect()
          : paneElement(layout.focused)?.getBoundingClientRect()
      const size = {
        width: from?.width ?? 480,
        height: from?.height ?? 360,
      }
      // Held where it was grabbed: a pane by its header, a card by its top.
      const home: Box =
        carried.kind === "pane" && from
          ? boxOf(from)
          : { left: x - size.width / 2, top: y - 20, ...size }
      // What the host covers the grid with is never a target.
      const covered = options.covered(scope).map(settledBox)
      const parts = new Map(
        Array.from(scope.querySelectorAll<HTMLElement>("[data-pane-key]"), (pane) => [
          pane,
          partsOf(pane),
        ]),
      )
      const motion = {
        duration: durationToken(scope, "--desktop-base"),
        easing: motionToken(scope, "--desktop-ease") ?? "ease",
      }
      // Only panes a person can see are aimed at: none under what covers them all.
      const targets: Targets | null = source.targetable()
        ? {
            grid,
            panes: [...boxes(layout, grid)],
            covered,
            refused: refusedZones(layout, carried, room),
          }
        : null
      const { ghost, inner } = ghostFor(carried, pressed, size)
      const grab = { x: x - home.left, y: y - home.top }
      const layer = document.createElement("div")
      layer.className = classes.layer
      layer.setAttribute("aria-hidden", "true")
      const carrier = document.createElement("div")
      carrier.className = classes.carrier
      carrier.style.transform = `translate(${x}px, ${y}px)`
      // The copy is drawn about its centre; the glider holds it where it was grabbed.
      const glider = document.createElement("div")
      glider.className = "split-panes-glider"
      glider.style.transform = `translate(${size.width / 2 - grab.x}px, ${size.height / 2 - grab.y}px)`
      // On the page, unseen, until the press becomes a drag.
      ghost.setAttribute(marks.waiting, "")
      glider.append(ghost)
      carrier.append(glider)
      layer.append(carrier)
      scope.append(layer)
      const shield = document.createElement("div")
      shield.className = classes.shield
      shield.setAttribute("aria-hidden", "true")
      return {
        layer,
        carrier,
        glider,
        ghost,
        inner,
        pressed,
        shield,
        home,
        grab,
        size,
        grid,
        room,
        targets,
        parts,
        motion,
        drawing: {
          pointer: { x, y },
          glide: null,
          fade: null,
          laid: size,
          shape: { from: size, to: size, motion: null, counter: null },
          frame: 0,
          still: 0,
          shown: null,
          outcome: null,
          outline: null,
          landing: null,
        },
      }
    }

    /** The press became a drag: the copy is shown under the pointer, and glides to its centre. */
    const begin = (made: Made, what: Carried, pointerId: number, at: PointerSample) => {
      const { carrier, glider, ghost, grab, size, shield, pressed, drawing } = made
      // Before the copy is shown. A backdrop blur would be sampled again on
      // every frame the copy moves (`styles.test.ts`).
      holdGlass(true)
      drawing.pointer = { x: at.x, y: at.y }
      shield.addEventListener("lostpointercapture", onLost)
      window.getSelection()?.removeAllRanges()
      // With the pointer from its first frame, and seen from it. The glass
      // blur is already off, so showing the copy does not sample it
      // (`styles.test.ts`).
      carrier.style.transform = `translate(${at.x}px, ${at.y}px)`
      ghost.removeAttribute(marks.waiting)
      drawing.glide = glider.animate(
        [
          {
            transform: `translate(${size.width / 2 - grab.x}px, ${size.height / 2 - grab.y}px)`,
          },
          { transform: "translate(0px, 0px)" },
        ],
        { ...made.motion, fill: "forwards" },
      )
      scope.append(shield)
      scope.setAttribute(marks.carrying, what.kind)
      pressed.setAttribute(marks.carrying, "")
      if (what.kind === "pane") paneElement(what.pane)?.setAttribute(marks.lifted, "")
      try {
        // The shield holds the pointer: its hand shows wherever the pointer goes.
        shield.setPointerCapture(pointerId)
      } catch {
        // A pointer the page no longer has: the drag still follows window events.
      }
      // The copy is painted in this frame; what the zone under the pointer
      // would do waits for the next, so neither frame does both.
      drawing.frame = requestAnimationFrame(() => {
        if (carried() === made) drawing.frame = requestAnimationFrame(show)
      })
      restLater(made, at.t)
    }

    /**
     * Once the pointer has been still for `restAfter` since `t` — its last
     * move, on the events' own clock — the zone is decided as at rest.
     */
    const restLater = (made: Made, t: number) => {
      window.clearTimeout(made.drawing.still)
      made.drawing.still = window.setTimeout(
        () => send({ kind: "still", t: t + restAfter, targets: made.targets }),
        restAfter,
      )
    }

    /**
     * Ends what the page marked for a drag. What it marked goes at once, or —
     * for a drop, `later` — a frame on, so the commit's own flight measures the
     * page as the preview left it rather than laying it out again first.
     * Whatever a drag left selected goes with it.
     */
    const tidy = (made: Made, later = false) => {
      const { drawing, pressed, shield } = made
      cancelAnimationFrame(drawing.frame)
      window.clearTimeout(drawing.still)
      shield.remove()
      window.getSelection()?.removeAllRanges()
      const { outline } = drawing
      const unmark = () => {
        outline?.remove()
        scope.removeAttribute(marks.carrying)
        pressed.removeAttribute(marks.carrying)
        scope
          .querySelectorAll(`[${marks.lifted}]`)
          .forEach((pane) => pane.removeAttribute(marks.lifted))
        // Blur and shadows come back; a flight of the drop's own holds them itself.
        // Bodies were held out of the preview and come back one a frame.
        releaseReflow()
        holdGlass(false)
        announcer.textContent = ""
      }
      // A frame after the one that commits the drop.
      if (later)
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            if (ownsDragResources(phase, made)) unmark()
          }),
        )
      else unmark()
    }

    /** Releases the copy and preview resources retained by this drag. */
    const releaseMade = (made: Made) => {
      made.drawing.fade?.cancel()
      made.drawing.glide?.cancel()
      made.drawing.shape.motion?.cancel()
      made.drawing.shape.counter?.cancel()
      ;[...previewed.keys()].forEach(letGo)
      letGoOfDragPreview(scope)
      previewed.clear()
      scope.removeAttribute(marks.takesSpare)
      tidy(made)
      made.layer.remove()
    }

    /** The copy's last flight ended: the drag is over. */
    const landed = (made: Made) => {
      if (!ownsDragResources(phase, made)) return
      releaseMade(made)
      send({ kind: "landed", made })
    }

    /**
     * Cancelled. Nothing around it changed (`home`): the copy flies home as
     * the panes go back, from where the pointer left it — known, not read.
     * The room, the panes or the view changed (`at-once`): the copy and the
     * preview go now, and the change plays as it would with no drag.
     * Flying home keeps the preview's cheap paint for two frames (`tidy`'s
     * `later`), the same handoff a drop uses, so this turn does not also
     * restore blur and shadows.
     */
    const cancel = (made: Made, how: "home" | "at-once") => {
      if (how === "at-once") {
        landed(made)
        return
      }
      preview(made, null, null)
      const back = flyTo(made, made.home, true)
      scope.removeAttribute(marks.takesSpare)
      // The preview's cheap paint stays two frames, the same handoff a drop
      // uses, so this turn does not also restore blur and shadows
      // (`drag.test.tsx`). A flight that finishes after a newer drag owns the
      // page leaves that drag alone (`split-panes-drag.test.tsx`).
      tidy(made, true)
      void back.finished
        .catch(() => undefined)
        .then(() => {
          if (!ownsDragResources(phase, made)) return
          releaseReflow()
          landed(made)
        })
    }

    /** Let go with a zone shown: the copy flies, and what is shown is committed on the next frame. */
    const drop = (made: Made, what: Carried, aim: Aim) => {
      const { ghost, room, drawing } = made
      // The copy flies from the pointer into the place it takes. That place
      // was drawn while carrying; the flight does not read the page.
      const flight = drawing.landing ? flyTo(made, drawing.landing, false) : null
      tidy(made, true)
      // The commit lays the panes out at their new sizes, and FlipScope reads
      // that layout in the same turn (`play`). On pointerup that read shared
      // the frame with this flight and ran past the frame budget. One frame
      // on, the preview is still up — tidy waits two — so the commit's flight
      // measures through it. `drag.test.tsx` holds that the commit is not in
      // the pointerup turn.
      // The frame commits the preview, not whatever the panes became while it
      // waited: a resize, a watched change, or a carried item let go. The
      // dropping phase ignores those, so the frame itself checks.
      const previewHolds = () => {
        const watched = source.watched()
        if (
          layoutNow()?.columns !== seen.columns ||
          watched.length !== seen.watched.length ||
          watched.some((value, index) => value !== seen.watched[index])
        )
          return false
        return what.kind !== "item" || source.holds(what.item)
      }
      let accept = true
      const refuse = () => {
        accept = false
      }
      window.addEventListener("resize", refuse)
      const unsubscribe = source.subscribe(() => {
        if (!previewHolds()) refuse()
      })
      const frame = requestAnimationFrame(() => {
        release()
        if (!ownsDragResources(phase, made)) return
        if (accept && previewHolds()) {
          // Bodies stay out of this layout and return one a frame (`drag.test.tsx`).
          scope.querySelectorAll<HTMLElement>("[data-pane-key]").forEach((pane) => {
            pane.setAttribute(marks.settling, "")
          })
          source.commitDrop({ carried: what, target: aim.target, zone: aim.zone, room })
          requestAnimationFrame(revealSettling)
          return
        }
        // The preview was the arrangement this frame is not committing.
        scope.removeAttribute(marks.takesSpare)
        letGoOfDragPreview(scope)
        previewed.clear()
      })
      const release = () => {
        cancelAnimationFrame(frame)
        window.removeEventListener("resize", refuse)
        unsubscribe()
        if (releaseDrop === release) releaseDrop = null
      }
      releaseDrop = release
      // Whatever `FlipScope` did not measure through — a drop that changed no
      // arrangement — lets go a frame on.
      const previewReleased = new Promise<void>((resolve) => {
        requestAnimationFrame(() =>
          requestAnimationFrame(() => {
            if (ownsDragResources(phase, made)) {
              scope.removeAttribute(marks.takesSpare)
              letGoOfDragPreview(scope)
              previewed.clear()
            }
            resolve()
          }),
        )
      })
      // The owner stays through the commit's frame even when its flight is instant.
      void Promise.all([
        (flight?.finished ?? Promise.resolve()).catch(() => undefined),
        previewReleased,
      ])
        .then(() => {
          if (!ownsDragResources(phase, made)) return
          // Gone at once. A fade would blend the copy with the blur after it
          // landed (`drag.test.tsx`).
          const fade = ghost.animate([{ opacity: 0.85 }, { opacity: 0 }], {
            duration: 0,
            fill: "forwards",
          })
          drawing.fade = fade
          return fade.finished.catch(() => undefined)
        })
        .then(() => landed(made))
    }

    /** A press no longer waiting to become a drag: its pending frame and task go. */
    const stopWaiting = () => {
      if (!waiting) return
      cancelAnimationFrame(waiting.frame)
      window.clearTimeout(waiting.timer)
      waiting = null
    }

    // The page going inert under something modal is seen as it happens, not at the next move.
    const inert = new MutationObserver(() => {
      if (scope.closest("[inert]")) send({ kind: "changed" })
    })

    // ——— Every event goes through the drag's phase ———

    const send = (event: DragEvent<Made>) => {
      const was = phase
      const next = stepDrag(was, event)
      if (next === was) return
      phase = next
      const live = next.kind === "pressed" || next.kind === "carrying"
      if (!live) {
        stopWaiting()
        if (next.kind !== "cancelling" || next.how !== "home") inert.disconnect()
      }
      // A press that ends before its copy was shown takes away what was made for it.
      if (was.kind === "pressed" && next.kind === "idle") was.made?.layer.remove()
      if (was.kind === "pressed" && next.kind === "carrying")
        begin(next.made, next.carried, next.pointerId, next.path[0])
      else if (was.kind === "carrying" && next.kind === "carrying") {
        const { drawing } = next.made
        const at = next.path.at(-1)
        if (at && (at.x !== drawing.pointer.x || at.y !== drawing.pointer.y)) {
          drawing.pointer = { x: at.x, y: at.y }
          // The copy belongs to the pointer, one to one — moved as the pointer
          // moves, not a frame later.
          next.made.carrier.style.transform = `translate(${at.x}px, ${at.y}px)`
          restLater(next.made, at.t)
        }
        if (!sameAim(next.aim, drawing.shown) && !drawing.frame)
          drawing.frame = requestAnimationFrame(show)
      } else if (was.kind === "carrying" && next.kind === "dropping")
        drop(was.made, next.carried, next.aim)
      else if (next.kind === "cancelling") cancel(next.made, next.how)
    }

    const sample = (event: PointerEvent): PointerSample => ({
      x: event.clientX,
      y: event.clientY,
      t: event.timeStamp,
    })

    /** Whether what the drag read still holds: the panes and what the host watches, as the press found them, and the item carried. */
    const unchanged = () => {
      if (phase.kind === "idle" || phase.kind === "dropping") return true
      const watched = source.watched()
      if (
        layoutNow()?.columns !== seen.columns ||
        watched.length !== seen.watched.length ||
        watched.some((value, index) => value !== seen.watched[index])
      )
        return false
      const { carried: what } = phase
      return what.kind !== "item" || source.holds(what.item)
    }

    const onPointerDown = (event: PointerEvent) => {
      // Another button of the carrying pointer reaches the page as a move
      // (a chord), never a press; a press of another pointer is not the drag's.
      if (event.button !== 0 || phase.kind !== "idle") return
      const target = event.target as Element
      const pressed = target.closest<HTMLElement>(
        `[${marks.dragPane}], [${marks.dragItem}]`,
      )
      if (!pressed || !scope.contains(pressed)) return
      // A control inside what is carried keeps its own press.
      const control = target.closest("button, input, textarea, a, [role='menuitem']")
      if (control && control !== pressed && pressed.contains(control)) return
      const pane = pressed.getAttribute(marks.dragPane) ?? undefined
      const item = pressed.getAttribute(marks.dragItem) ?? undefined
      const what: Carried | null =
        pane !== undefined
          ? { kind: "pane", pane: Number(pane) }
          : item
            ? { kind: "item", item }
            : null
      if (!what) return
      // An untrusted press accepted here is cancelled. Chrome, given a scripted
      // pointerdown that is not, then a move and a lift in that same task, sets
      // `pointer-events: none` on the body a few frames later (`drag.mjs` flick).
      // `drag.test.tsx` holds the cancel. A trusted press is left to the browser:
      // cancelling it would swallow the click a session row is opened by.
      if (!event.isTrusted) event.preventDefault()
      seen = { columns: layoutNow()?.columns, watched: source.watched() }
      const { clientX: x, clientY: y } = event
      send({
        kind: "press",
        carried: what,
        pointerId: event.pointerId,
        at: sample(event),
      })
      inert.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["inert"],
        subtree: true,
      })
      // Made in a task once the page has painted this frame — its reads then
      // find it laid out, and the press's own frame does nothing more. Until
      // it is made, a move does not lift: no pointer event ever reads the page.
      const pending = { frame: 0, timer: 0 }
      waiting = pending
      pending.frame = requestAnimationFrame(() => {
        pending.timer = window.setTimeout(() => {
          if (waiting !== pending) return
          waiting = null
          if (phase.kind !== "pressed" || phase.made) return
          const made = prepare(what, pressed, x, y)
          // Nothing to carry on this page (no panes laid out): the press ends.
          send(made ? { kind: "ready", made } : { kind: "changed" })
        }, 0)
      })
    }

    const onPointerMove = (event: PointerEvent) => {
      if (phase.kind !== "pressed" && phase.kind !== "carrying") return
      const wasPressed = phase.kind === "pressed"
      send({
        kind: "move",
        pointerId: event.pointerId,
        buttons: event.buttons,
        at: sample(event),
        targets: carried()?.targets ?? null,
      })
      // A drag, not a text selection or a window move.
      if (wasPressed && phase.kind === "carrying") event.preventDefault()
    }

    // A press on something that can be carried, and the drag it becomes,
    // select nothing: the browser's own selection is stopped as it starts, in
    // both engines, without restyling the page.
    const onSelectStart = (event: Event) => {
      if (phase.kind === "pressed" || phase.kind === "carrying") event.preventDefault()
    }

    const swallowClick = (event: MouseEvent) => {
      event.stopPropagation()
      event.preventDefault()
    }

    const onPointerUp = (event: PointerEvent) => {
      if (phase.kind === "pressed") {
        send({
          kind: "release",
          pointerId: event.pointerId,
          at: sample(event),
          targets: null,
          shown: null,
        })
        return
      }
      if (phase.kind !== "carrying" || event.pointerId !== phase.pointerId) return
      // The press that ended a drag is not also a click on what it started from.
      window.addEventListener("click", swallowClick, { capture: true, once: true })
      window.setTimeout(() => window.removeEventListener("click", swallowClick, true), 0)
      const { drawing, targets } = phase.made
      send({
        kind: "release",
        pointerId: event.pointerId,
        at: sample(event),
        targets,
        // What the page shows, and offers something: what the person saw.
        shown: drawing.outcome ? drawing.shown : null,
      })
    }

    // A pointer the page loses hold of — taken by the system, or gone with
    // the window — ends the press or the drag as Escape does, and a later
    // move starts nothing.
    const onLost = () => send({ kind: "lost" })

    const onKeyDown = (event: KeyboardEvent) => {
      if (phase.kind === "idle" || phase.kind === "dropping") return
      // Under something modal the keys are its own, its Escape included.
      if (scope.closest("[inert]")) return
      const said = keyToDrag(event.key)
      if (!said) return
      if (said.kind === "escape" && phase.kind === "carrying") {
        event.preventDefault()
        event.stopPropagation()
      }
      send(said)
    }

    const onChange = () => send({ kind: "changed" })
    const onStore = () => {
      if (!unchanged()) onChange()
    }
    const unsubscribe = source.subscribe(onStore)

    scope.addEventListener("pointerdown", onPointerDown)
    window.addEventListener("pointermove", onPointerMove)
    window.addEventListener("pointerup", onPointerUp)
    window.addEventListener("pointercancel", onLost)
    window.addEventListener("blur", onLost)
    window.addEventListener("resize", onChange)
    window.addEventListener("keydown", onKeyDown, true)
    document.addEventListener("selectstart", onSelectStart)
    return () => {
      unsubscribe()
      inert.disconnect()
      scope.removeEventListener("pointerdown", onPointerDown)
      window.removeEventListener("pointermove", onPointerMove)
      window.removeEventListener("pointerup", onPointerUp)
      window.removeEventListener("pointercancel", onLost)
      window.removeEventListener("blur", onLost)
      window.removeEventListener("resize", onChange)
      window.removeEventListener("keydown", onKeyDown, true)
      document.removeEventListener("selectstart", onSelectStart)
      stopWaiting()
      releaseDrop?.()
      const made = phase.kind === "idle" ? null : phase.made
      if (made) releaseMade(made)
      phase = idle
      announcer.remove()
    }
  }, [root, source, options])
}
