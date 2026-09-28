/**
 * Drag and drop in the workspace, carried by the pointer rather than the
 * browser's own drag, so what is carried looks like the window it will be:
 *
 * - a pane lifted by its header, or a session taken from a list, becomes a
 *   translucent copy at a pane's size (the pane's own; for a session, the
 *   focused pane's), held where it was grabbed;
 * - over a pane, the zone under the pointer — a side, or the middle — shows
 *   what dropping there would do: the panes move to where the drop would put
 *   them and the copy settles into its place, from the one outcome the
 *   command will commit (`model/drop.ts`, asked through `previewDrop`); a
 *   zone the room refuses shows nothing;
 * - let go over a zone, the command commits that outcome, and — the panes
 *   already there — nothing jumps; let go anywhere else, or Escape, and the
 *   copy flies home as the panes go back.
 *
 * Everything moves by transform, in animations the compositor runs; the
 * pointer is followed a frame at a time without rendering anything, and the
 * store is only asked when the zone changes. No frame of a drag makes the page
 * lay out early: the copy is made once the press's frame has painted — one screen of it, in
 * a box of its own size that lays out nothing else — and what a drag needs of
 * the page (the grid, each pane's parts) is read then too, so beginning,
 * previewing, dropping and letting go only write. Where a preview has drawn a
 * pane is known from the preview's own motion, never read back. The preview's motion is marked
 * (`dragPreview`) so `FlipScope` measures through it at the drop and lets it
 * go before it measures where things landed. With less motion, nothing
 * moves: the copy follows the pointer and the place it would land is
 * outlined. The zone is said to assistive technology as it changes.
 *
 * A component takes part by marking what can be carried: `data-drag-pane`
 * (a pane's key, on its header) or `data-drag-session` (a session's id, on a
 * row). The keyboard's own ways — the Move items, ⌃⌥ and an arrow — are
 * untouched.
 */
import { useEffect, type RefObject } from "react"
import { reducedMotion } from "../../../adapters/motion-preference"
import type { DesktopStore } from "../../../store"
import type { Carried, DropOutcome } from "../../model/drop"
import {
  aimPoint,
  headingWindow,
  paneAt,
  pointerVelocity,
  zoneAt,
  type PointerSample,
} from "../../model/drop"
import type { PaneKey, PaneLayout, Zone } from "../../model/pane-layout"
import { paneLimits } from "../../model/pane-layout"
import { placements, type PanePlacement } from "../../model/pane-sizing"
import { dropSession, movePane, previewDrop } from "../store/commands"
import { durationToken, motionToken } from "../../../adapters/motion"

/** Marks the preview's own motion. */
const dragPreview = "workspace-drag-preview"

/** The preview's animations under each workspace root, so they can be let go without a search. */
const previews = new WeakMap<Element, Set<Animation>>()

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

const inPlace: Drawn = { dx: 0, dy: 0, sx: 1, sy: 1, opacity: 1 }

const transformOf = ({ dx, dy, sx, sy }: Drawn) =>
  `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`

/** Part of the way from one drawing to another, as a transform's functions interpolate. */
function partWay(from: Drawn, to: Drawn, progress: number): Drawn {
  const at = (a: number, b: number) => a + (b - a) * progress
  return {
    dx: at(from.dx, to.dx),
    dy: at(from.dy, to.dy),
    sx: at(from.sx, to.sx),
    sy: at(from.sy, to.sy),
    opacity: at(from.opacity, to.opacity),
  }
}

/**
 * Lets go of a drag's preview under `root`: what `FlipScope` does before it
 * measures where a drop landed, so a drop that lands where it previewed has
 * nowhere to fly. Nothing to do, and no search, when no drag previewed.
 */
export function letGoOfDragPreview(root: Element): void {
  const held = previews.get(root)
  if (!held) return
  held.forEach((animation) => animation.cancel())
  previews.delete(root)
}

/** How far the pointer moves, pressed, before a press becomes a drag. */
const threshold = 4

/** How many of a session's latest messages its copy shows: a screen's worth. */
const latestShown = 12

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

/** What the zone does, as it is said. */
function saying(outcome: DropOutcome, zone: Zone, title: string): string {
  switch (outcome.does) {
    case "swap":
      return `Swap with ${title}`
    case "move":
      return `Move ${zone === "top" ? "above" : zone === "bottom" ? "below" : `${zone} of`} ${title}`
    case "replace":
      return `Open in place of ${title}`
    case "split":
      return `Split ${zone === "top" ? "above" : zone === "bottom" ? "below" : zone} of ${title}`
    case "go-to":
      return `Go to ${title}`
  }
}

export function useWorkspaceDrag(
  store: DesktopStore,
  root: RefObject<HTMLElement | null>,
): void {
  useEffect(() => {
    const scope = root.current
    if (!scope) return

    const announcer = document.createElement("div")
    announcer.className = "workspace-visually-hidden"
    announcer.setAttribute("role", "status")
    announcer.setAttribute("aria-live", "polite")
    scope.append(announcer)

    /**
     * What a drag needs, made once the press's frame has painted, while the page is laid
     * out and nothing has been written: the copy, hidden, in a box of its own
     * size; where it starts; and what the drag reads of the page — the grid,
     * each pane's parts, the sidebar's room — so no frame of it reads again.
     */
    interface Prepared {
      readonly ghost: HTMLElement
      /** The copy's content, counter-scaled as its box settles into a place. */
      readonly inner: HTMLElement
      /** Where the copy started, and flies back to. */
      readonly home: Box
      readonly grab: { x: number; y: number }
      readonly size: { width: number; height: number }
      readonly grid: Box
      /** What the sidebar gives up if the drop folds it. */
      readonly spare: number
      /** Each pane's parts and where each sits in it: what a preview counter-scales. */
      readonly parts: ReadonlyMap<
        HTMLElement,
        readonly { element: HTMLElement; top: number }[]
      >
      readonly motion: { duration: number; easing: string }
    }

    let pressed: {
      carried: Carried
      source: HTMLElement
      x: number
      y: number
      pointer: number
      /** Made after the press's frame has painted (`onPointerDown`), or as it becomes a drag. */
      prepared: Prepared | null
    } | null = null
    let drag:
      | (Prepared & {
          carried: Carried
          source: HTMLElement
          pointer: { x: number; y: number }
          /** Where the pointer has been lately, for where it heads (`pointerVelocity`). */
          path: PointerSample[]
          frame: number
          aim: { target: PaneKey; zone: Zone } | null
          outcome: DropOutcome | null
          outline: HTMLElement | null
          /** Where the drop would land, if it would. */
          landing: Box | null
          /** Over the page while carrying: the grabbing hand, and nothing under it selected. */
          shield: HTMLElement
        })
      | null = null

    const layoutNow = () => store.getState().workspace.panes
    const readGrid = (): Box | null => {
      const grid = scope.querySelector<HTMLElement>(".workspace-panes")
      if (!grid) return null
      const rect = grid.getBoundingClientRect()
      return {
        left: rect.left,
        top: rect.top,
        width: grid.offsetWidth,
        height: grid.offsetHeight,
      }
    }
    const paneElement = (key: PaneKey) =>
      scope.querySelector<HTMLElement>(`[data-pane-key="${key}"]`)
    const timing = () => drag?.motion ?? { duration: 0, easing: "ease" }

    // ——— The preview: panes where the drop would put them ———

    // Each pane's preview, as it was asked for: where it is drawn is where
    // this says, part way on its clock — never read back from the page.
    const previewed = new Map<
      HTMLElement,
      { from: Drawn; to: Drawn; motion: Animation; parts: Animation[] }
    >()
    const drawnNow = (pane: HTMLElement): Drawn => {
      const held = previewed.get(pane)
      const progress = held?.motion.effect?.getComputedTiming().progress
      if (!held || progress === null || progress === undefined) return inPlace
      return partWay(held.from, held.to, progress)
    }
    const letGo = (pane: HTMLElement) => {
      const held = previewed.get(pane)
      if (!held) return
      previewed.delete(pane)
      for (const animation of [held.motion, ...held.parts]) {
        animation.cancel()
        previews.get(scope)?.delete(animation)
      }
    }

    /**
     * Draws a pane laid out at `real` at `to` — or where it is laid out — from
     * where it is drawn now. It only writes.
     */
    const reflow = (pane: HTMLElement, real: Box, to: Box | null, fade = false) => {
      if (!drag) return
      const from = drawnNow(pane)
      const target = to ? between(real, to, fade ? 0.2 : 1) : inPlace
      letGo(pane)
      const parts = drag.parts.get(pane) ?? []
      pane.style.transformOrigin = "50% 50%"
      const motion = track(
        scope,
        pane.animate(
          [
            { transform: transformOf(from), opacity: from.opacity },
            { transform: transformOf(target), opacity: target.opacity },
          ],
          { ...timing(), fill: "forwards", id: dragPreview },
        ),
      )
      // The content keeps its size, as in a flight (`flip.tsx`), held to the
      // pane's top left as it waits there, so it reads from its start.
      const counter = parts.map(({ element, top }) => {
        element.style.transformOrigin = `0px ${-top}px`
        return track(
          scope,
          element.animate(
            [
              { transform: `scale(${1 / from.sx}, ${1 / from.sy})` },
              { transform: `scale(${1 / target.sx}, ${1 / target.sy})` },
            ],
            { ...timing(), fill: "forwards", id: dragPreview },
          ),
        )
      })
      previewed.set(pane, { from, to: target, motion, parts: counter })
    }

    /** The transforms that draw the copy — laid out at its own size — at `box`, its content unstretched. */
    const drawnAt = (box: Box) => {
      if (!drag) return { outer: "none", inner: "none" }
      const { size } = drag
      const sx = box.width / size.width
      const sy = box.height / size.height
      return {
        outer: `translate(${box.left}px, ${box.top}px) scale(${sx}, ${sy})`,
        inner: `scale(${1 / sx}, ${1 / sy})`,
      }
    }

    /**
     * The copy flies into `box` — on a drop — by transform alone, from
     * wherever it is drawn now. Its box is scaled and
     * its content counter-scaled, so its words keep their size and nothing is
     * laid out again, as a pane's flight does (`flip.tsx`).
     */
    const settleGhost = (box: Box) => {
      if (!drag) return null
      const { ghost, inner } = drag
      // With the pointer until now, it has no motion of its own to read back.
      const end = drawnAt(box)
      const options: KeyframeAnimationOptions = { ...timing(), fill: "forwards" }
      inner.animate([{ transform: "none" }, { transform: end.inner }], options)
      return ghost.animate(
        [{ transform: ghost.style.transform || "none" }, { transform: end.outer }],
        options,
      )
    }

    /**
     * The calm placeholder where the drop would land: a soft fill and a
     * hairline in the theme's edge light, at exactly the rect the pane will
     * take. It moves between zones on its own short transition; the copy
     * carried stays with the pointer.
     */
    const placeholder = (box: Box | null) => {
      if (!drag) return
      drag.landing = box
      if (!box) {
        drag.outline?.remove()
        drag.outline = null
        return
      }
      const fresh = !drag.outline
      const element = drag.outline ?? document.createElement("div")
      element.className = "workspace-drag-placeholder"
      Object.assign(element.style, {
        transform: `translate(${box.left}px, ${box.top}px)`,
        width: `${box.width}px`,
        height: `${box.height}px`,
      })
      if (fresh) scope.append(element)
      drag.outline = element
    }

    /** Shows what dropping at `aim` would do, or — with no aim — nothing. It only writes. */
    const preview = (
      outcome: DropOutcome | null,
      target: PaneKey | null,
      zone: Zone | null,
    ) => {
      const layout = layoutNow()
      if (!drag || !layout) return
      const { grid } = drag
      const real = boxes(layout, grid)
      const spare = outcome?.foldSidebar ? drag.spare : 0
      const landing = outcome ? boxes(outcome.layout, landingGrid(grid, spare)) : null
      if (scope.hasAttribute("data-drag-folds") !== Boolean(outcome?.foldSidebar))
        scope.toggleAttribute("data-drag-folds", Boolean(outcome?.foldSidebar))
      if (outcome && landing && !("dragReflow" in scope.dataset))
        scope.dataset.dragReflow = ""
      if (!reducedMotion())
        for (const [key, box] of real) {
          const pane = paneElement(key)
          if (!pane) continue
          const to = landing?.get(key) ?? null
          const replaced = outcome?.does === "replace" && key === outcome.lands
          reflow(
            pane,
            box,
            outcome && to ? inside(to, landingGrid(grid, spare)) : null,
            replaced,
          )
        }
      const lands = outcome && landing ? (landing.get(outcome.lands) ?? null) : null
      placeholder(lands && inside(lands, landingGrid(grid, spare)))
      announcer.textContent =
        outcome && target !== null && zone
          ? saying(
              outcome,
              zone,
              paneElement(target)?.getAttribute("aria-label") ?? "the pane",
            )
          : ""
    }

    /** The grid a drop would leave, the sidebar's room added when it folds. */
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

    // ——— Following the pointer ———

    /**
     * Whether the pointer is over the pane grid. Off it — over a side column,
     * or out of the window — a drag aims at nothing, and let go there it goes
     * back: what is shown is what a release does.
     */
    const inGrid = ({ x, y }: { x: number; y: number }) =>
      drag !== null &&
      x >= drag.grid.left &&
      x <= drag.grid.left + drag.grid.width &&
      y >= drag.grid.top &&
      y <= drag.grid.top + drag.grid.height

    /**
     * The pane and zone the drag aims at, from where the panes are laid out,
     * not drawn: aimed between the pointer and the copy's centre
     * (`aimPoint`), a gutter or a place past the grid reading as the pane
     * nearest it (`paneAt`); the zone weighed by where the pointer heads, and
     * held while the aim stays on the same pane (`zoneAt`). Aimed at the
     * carried pane's own place, the drop offers nothing (`dropOutcome`): let
     * go there, it goes back.
     */
    const aimAt = (
      pointer: { x: number; y: number },
      velocity: { x: number; y: number },
      was: { target: PaneKey; zone: Zone } | null,
    ): { target: PaneKey; zone: Zone } | null => {
      const layout = layoutNow()
      if (!layout || !drag || !inGrid(pointer)) return null
      const { grab, size } = drag
      const aim = aimPoint(pointer, {
        left: pointer.x - grab.x,
        top: pointer.y - grab.y,
        ...size,
      })
      const over = paneAt(aim, boxes(layout, drag.grid))
      if (!over) return null
      return {
        target: over.key,
        zone: zoneAt(
          { x: over.x, y: over.y, velocity },
          over.box,
          was?.target === over.key ? was.zone : null,
        ),
      }
    }

    const follow = () => {
      if (!drag) return
      drag.frame = 0
      const { pointer } = drag
      const aim = aimAt(pointer, pointerVelocity(drag.path), drag.aim)
      const same = aim?.target === drag.aim?.target && aim?.zone === drag.aim?.zone
      if (!same) {
        drag.aim = aim
        const outcome = aim
          ? store.dispatch(
              previewDrop({ carried: drag.carried, target: aim.target, zone: aim.zone }),
            )
          : null
        // A pane over itself, or a zone the room refuses: nothing is offered.
        const changed = outcome !== drag.outcome
        drag.outcome = outcome
        if (changed) preview(outcome, aim?.target ?? null, aim?.zone ?? null)
      }
    }

    /**
     * The copy belongs to the pointer, one to one, the whole drag — moved as
     * the pointer moves, not a frame later: where it would land is the
     * placeholder's to show, not the copy's.
     */
    const carry = () => {
      if (!drag) return
      const { pointer, grab, ghost } = drag
      ghost.style.transform = `translate(${pointer.x - grab.x}px, ${pointer.y - grab.y}px)`
    }

    const schedule = () => {
      if (drag && !drag.frame) drag.frame = requestAnimationFrame(follow)
    }

    // ——— Beginning, ending ———

    /**
     * The copy that is carried, made once the press's frame has painted and never rendered
     * again: the pane itself — its header, its conversation where it was
     * scrolled to, its composer and chips — or, for a session taken from a
     * list, its heading and latest words as the window holds them, over the
     * focused pane's composer. A picture, not a control: nothing in it takes
     * focus, a pointer, or a name. Made whole before it is on the page — what
     * is typed filled in, the conversation moved to where it was scrolled by
     * transform — so putting it there lays out one box of its own size and
     * nothing else, in the frame's own layout.
     */
    const ghostFor = (
      carried: Carried,
      source: HTMLElement,
      size: { width: number; height: number },
    ) => {
      const ghost = document.createElement("div")
      ghost.className = "workspace-drag-ghost"
      ghost.setAttribute("aria-hidden", "true")
      ghost.inert = true
      Object.assign(ghost.style, { width: `${size.width}px`, height: `${size.height}px` })
      const inner = document.createElement("div")
      inner.className = "workspace-drag-ghost-inner"
      ghost.append(inner)
      const picture = (
        element: Element,
        prune?: (from: Element, copy: Element) => void,
      ): HTMLElement => {
        const clone = element.cloneNode(true) as HTMLElement
        prune?.(element, clone)
        for (const node of [clone, ...clone.querySelectorAll<HTMLElement>("*")]) {
          for (const name of [
            "id",
            "tabindex",
            "aria-label",
            "aria-labelledby",
            "aria-describedby",
            "data-pane-key",
            "data-flip",
            "data-flip-id",
            "data-drag-pane",
            "data-tauri-drag-region",
            "data-lifted",
            "data-pane-focused",
          ])
            node.removeAttribute(name)
        }
        return clone
      }
      /**
       * The conversation as it shows, and no more: the copy is a picture of
       * one screen of it, so what is scrolled out of view above stands in as
       * one spacer of its height and what is below is left out, and the copy
       * is moved up by where it was scrolled to — a transform, never a scroll,
       * which would lay the copy out at once.
       */
      const onScreenOnly = (from: Element, to: Element) => {
        const scroller = from.querySelector<HTMLElement>(".workspace-transcript")
        const content = scroller?.querySelector(":scope > .workspace-transcript-inner")
        const copied = to.querySelector<HTMLElement>(
          ".workspace-transcript > .workspace-transcript-inner",
        )
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
          copy.classList.add("workspace-drag-ghost-pane")
          const typed = pane.querySelectorAll("textarea")
          copy.querySelectorAll("textarea").forEach((field, index) => {
            field.value = typed[index]?.value ?? ""
          })
          inner.append(copy)
          return { ghost, inner }
        }
      }
      // A session with no pane yet: its heading and latest words, as the window holds them.
      const card = document.createElement("article")
      card.className = "workspace-pane workspace-drag-ghost-pane"
      const header = document.createElement("header")
      header.className = "workspace-pane-header"
      const tile = source.querySelector(".workspace-agent-tile")
      if (tile) header.append(picture(tile))
      const body = document.createElement("div")
      body.className = "workspace-pane-body"
      const transcript = document.createElement("div")
      // Its latest words at the foot, as a conversation opens: set by the copy's
      // own layout, not a scroll (`chrome.css`).
      transcript.className = "workspace-transcript workspace-drag-ghost-latest"
      const content = document.createElement("div")
      content.className = "workspace-transcript-inner"
      const heading = document.createElement("div")
      heading.className = "workspace-heading"
      const title = document.createElement("h2")
      const sessionId = carried.kind === "session" ? carried.sessionId : ""
      const state = store.getState().workspace
      const summary = Object.hasOwn(state.sessions, sessionId)
        ? state.sessions[sessionId]
        : undefined
      title.textContent = summary?.title ?? source.textContent ?? ""
      heading.append(title)
      content.append(heading)
      const held = Object.hasOwn(state.transcripts, sessionId)
        ? state.transcripts[sessionId]
        : undefined
      // A screen's worth: the latest few, which is all the copy shows.
      const messages = (held?.messages ?? []).slice(-latestShown)
      if (messages.length === 0 && summary?.preview) {
        const line = document.createElement("p")
        line.className = "workspace-message"
        line.textContent = summary.preview
        content.append(line)
      }
      for (const message of messages) {
        const row = document.createElement("div")
        row.className = "workspace-message"
        row.dataset.role = message.role
        const text = message.parts
          .map((part) =>
            part.kind === "text" ? part.text : part.kind === "code" ? part.code : "",
          )
          .filter(Boolean)
          .join("\n\n")
        const block = document.createElement(message.role === "user" ? "div" : "p")
        if (message.role === "user") block.className = "workspace-bubble"
        block.textContent = text
        row.append(block)
        content.append(row)
      }
      transcript.append(content)
      body.append(transcript)
      // The composer it will have: the focused pane's, as it stands.
      const layout = layoutNow()
      const dock = layout
        ? paneElement(layout.focused)?.querySelector(
            ".workspace-dock, .workspace-pane-home .desktop-composer",
          )
        : null
      if (dock) {
        const composer = picture(dock)
        composer.classList.add("workspace-dock")
        composer.querySelectorAll("textarea").forEach((field) => (field.value = ""))
        body.append(composer)
      }
      card.append(header, body)
      inner.append(card)
      return { ghost, inner }
    }

    /**
     * Everything a drag will need, read and made after the press's frame — while
     * the page is still laid out — the copy put on the page unseen. A press
     * that never becomes a drag takes it away again.
     */
    const prepare = (carried: Carried, source: HTMLElement, x: number, y: number) => {
      const layout = layoutNow()
      const grid = readGrid()
      if (!layout || !grid) return null
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
      const sidebar =
        scope.dataset.sidebar === "open"
          ? scope.querySelector<HTMLElement>(".workspace-sidebar")
          : null
      const parts = new Map(
        Array.from(scope.querySelectorAll<HTMLElement>("[data-pane-key]"), (pane) => [
          pane,
          Array.from(pane.children, (child) => ({
            element: child as HTMLElement,
            top: (child as HTMLElement).offsetTop,
          })),
        ]),
      )
      const motion = {
        duration: durationToken(scope, "--desktop-base"),
        easing: motionToken(scope, "--desktop-ease") ?? "ease",
      }
      const { ghost, inner } = ghostFor(carried, source, size)
      // On the page, unseen, until the press becomes a drag.
      ghost.dataset.waiting = ""
      ghost.style.transform = `translate(${home.left}px, ${home.top}px)`
      scope.append(ghost)
      return {
        ghost,
        inner,
        home,
        grab: { x: x - home.left, y: y - home.top },
        size,
        grid,
        spare: sidebar ? sidebar.offsetWidth + paneLimits.gutter : 0,
        parts,
        motion,
      }
    }

    const begin = (event: PointerEvent) => {
      if (!pressed) return
      pressed.prepared ??= prepare(pressed.carried, pressed.source, pressed.x, pressed.y)
      if (!pressed.prepared) return
      const { carried, source, prepared } = pressed
      const shield = document.createElement("div")
      shield.className = "workspace-drag-shield"
      shield.setAttribute("aria-hidden", "true")
      drag = {
        ...prepared,
        carried,
        source,
        pointer: { x: event.clientX, y: event.clientY },
        path: [{ x: event.clientX, y: event.clientY, t: event.timeStamp }],
        frame: 0,
        aim: null,
        outcome: null,
        outline: null,
        landing: null,
        shield,
      }
      pressed.prepared = null
      shield.addEventListener("lostpointercapture", onLostCapture)
      window.getSelection()?.removeAllRanges()
      // With the pointer from its first frame, and seen from it.
      carry()
      delete prepared.ghost.dataset.waiting
      scope.append(shield)
      scope.dataset.dragging = carried.kind
      source.dataset.dragging = ""
      if (carried.kind === "pane")
        paneElement(carried.pane)?.setAttribute("data-lifted", "")
      try {
        // The shield holds the pointer: its hand shows wherever the pointer goes.
        shield.setPointerCapture(pressed.pointer)
      } catch {
        // A pointer the page no longer has: the drag still follows window events.
      }
      // The copy is painted in this frame; what the zone under the pointer
      // would do waits for the next, so neither frame does both.
      const first = drag
      first.frame = requestAnimationFrame(() => {
        if (drag === first) first.frame = requestAnimationFrame(follow)
      })
    }

    /**
     * Ends the drag. What it marked on the page goes at once, or — for a drop,
     * `later` — a frame on, so the commit measures the page as the preview
     * left it rather than laying it out again first. Whatever a drag left
     * selected goes with it.
     */
    const tidy = (later = false) => {
      if (!drag) return
      const { frame, outline, source, shield } = drag
      drag = null
      cancelAnimationFrame(frame)
      shield.remove()
      window.getSelection()?.removeAllRanges()
      const unmark = () => {
        outline?.remove()
        delete scope.dataset.dragging
        delete source.dataset.dragging
        scope
          .querySelectorAll("[data-lifted]")
          .forEach((pane) => pane.removeAttribute("data-lifted"))
        // Blur and shadows come back; a flight of the drop's own holds them itself.
        delete scope.dataset.dragReflow
        announcer.textContent = ""
      }
      // A frame after the one that commits the drop.
      if (later) requestAnimationFrame(() => requestAnimationFrame(unmark))
      else unmark()
    }

    /**
     * Let go somewhere that is not a zone, or Escape: the copy flies home as
     * the panes go back — from where the pointer left it, which is known, not
     * read back from the page.
     */
    const cancel = () => {
      if (!drag) return
      const { ghost, inner, home, size } = drag
      preview(null, null, null)
      const back = ghost.animate(
        [
          { transform: ghost.style.transform || "none", opacity: 1 },
          {
            transform: `translate(${home.left}px, ${home.top}px) scale(${home.width / size.width}, ${home.height / size.height})`,
            opacity: 0,
          },
        ],
        { ...timing(), fill: "forwards" },
      )
      inner.animate(
        [
          { transform: "none" },
          {
            transform: `scale(${size.width / home.width}, ${size.height / home.height})`,
          },
        ],
        { ...timing(), fill: "forwards" },
      )
      const panes = [...previewed.keys()]
      scope.removeAttribute("data-drag-folds")
      tidy()
      void back.finished
        .catch(() => undefined)
        .then(() => {
          ghost.remove()
          panes.forEach(letGo)
          // Back where they were: blur and shadows return, unless a drag began meanwhile.
          if (!drag) delete scope.dataset.dragReflow
        })
    }

    /** Let go over a zone: the command commits what was previewed, and the copy hands over. */
    const drop = () => {
      if (!drag) return
      const { carried, aim, outcome, ghost, landing } = drag
      if (!aim || !outcome) return cancel()
      // The command first, while the page is as the last frame laid it out,
      // so its measure of the room reads a clean page; the copy's flight and
      // the tidying are written after.
      if (carried.kind === "pane")
        store.dispatch(
          movePane({ pane: carried.pane, target: aim.target, zone: aim.zone }),
        )
      else
        store.dispatch(
          dropSession({
            sessionId: carried.sessionId,
            target: aim.target,
            zone: aim.zone,
          }),
        )
      // Now it snaps: the copy flies from the pointer into the place it takes.
      const flight = landing ? settleGhost(landing) : null
      tidy(true)
      // Whatever `FlipScope` did not measure through — a drop that changed no
      // arrangement — lets go a frame on.
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          // The side columns' preview of a fold, measured through by the commit, goes now.
          scope.removeAttribute("data-drag-folds")
          letGoOfDragPreview(scope)
          previewed.clear()
        }),
      )
      // Landed, it hands over to the real pane.
      void (flight?.finished ?? Promise.resolve())
        .catch(() => undefined)
        .then(() => {
          const fade = ghost.animate([{ opacity: 0.85 }, { opacity: 0 }], {
            duration: reducedMotion() ? 0 : 120,
            fill: "forwards",
          })
          return fade.finished.catch(() => undefined)
        })
        .then(() => ghost.remove())
    }

    /** A press that did not become a drag: what was made for one goes. */
    const release = () => {
      pressed?.prepared?.ghost.remove()
      pressed = null
    }

    // ——— The pointer ———

    const onPointerDown = (event: PointerEvent) => {
      if (event.button !== 0 || drag) return
      const target = event.target as Element
      const source = target.closest<HTMLElement>("[data-drag-pane], [data-drag-session]")
      if (!source || !scope.contains(source)) return
      // A control inside what is carried keeps its own press.
      const control = target.closest("button, input, textarea, a, [role='menuitem']")
      if (control && control !== source && source.contains(control)) return
      const pane = source.dataset.dragPane
      const session = source.dataset.dragSession
      const carried: Carried | null =
        pane !== undefined
          ? { kind: "pane", pane: Number(pane) }
          : session
            ? { kind: "session", sessionId: session }
            : null
      if (!carried) return
      release()
      const press = {
        carried,
        source,
        x: event.clientX,
        y: event.clientY,
        pointer: event.pointerId,
        prepared: null as Prepared | null,
        waiting: 0,
      }
      pressed = press
      // Made once the page has painted this frame and before anything writes
      // to it again — its reads then find it laid out, and the press's own
      // frame does nothing more. A press that becomes a drag sooner makes it
      // then (`begin`).
      press.waiting = requestAnimationFrame(() => {
        press.waiting = window.setTimeout(() => {
          press.waiting = 0
          if (pressed === press && !drag)
            press.prepared = prepare(carried, source, press.x, press.y)
        }, 0)
      })
    }

    const onPointerMove = (event: PointerEvent) => {
      if (drag) {
        drag.pointer = { x: event.clientX, y: event.clientY }
        // Only the last tenth of a second or so is read; a little more is kept.
        drag.path = [
          ...drag.path.filter(
            (sample) => event.timeStamp - sample.t <= 2 * headingWindow,
          ),
          { x: event.clientX, y: event.clientY, t: event.timeStamp },
        ]
        carry()
        schedule()
        return
      }
      if (!pressed) return
      if (Math.hypot(event.clientX - pressed.x, event.clientY - pressed.y) < threshold)
        return
      // A drag, not a text selection or a window move.
      event.preventDefault()
      begin(event)
    }

    // A press on something that can be carried, and the drag it becomes,
    // select nothing: the browser's own selection is stopped as it starts, in
    // both engines, without restyling the page.
    const onSelectStart = (event: Event) => {
      if (pressed || drag) event.preventDefault()
    }

    const swallowClick = (event: MouseEvent) => {
      event.stopPropagation()
      event.preventDefault()
    }

    const onPointerUp = (event: PointerEvent) => {
      release()
      if (!drag) return
      // The press that ended a drag is not also a click on what it started from.
      window.addEventListener("click", swallowClick, { capture: true, once: true })
      window.setTimeout(() => window.removeEventListener("click", swallowClick, true), 0)
      // Let go where the pointer is now, not where a frame last looked: the
      // zone under the release is the one committed — off the grid, none, and
      // it goes back (`aimAt`) — a frame still waiting to be read or not.
      drag.pointer = { x: event.clientX, y: event.clientY }
      follow()
      drop()
    }

    // A pointer the page loses hold of — taken by the system, or gone with
    // the window — ends the drag as Escape does: nothing is dropped.
    const onLostCapture = () => {
      if (drag) cancel()
    }

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !drag) return
      event.preventDefault()
      event.stopPropagation()
      release()
      cancel()
    }

    const onCancel = () => {
      release()
      cancel()
    }

    scope.addEventListener("pointerdown", onPointerDown)
    window.addEventListener("pointermove", onPointerMove)
    window.addEventListener("pointerup", onPointerUp)
    window.addEventListener("pointercancel", onCancel)
    window.addEventListener("keydown", onKeyDown, true)
    window.addEventListener("blur", onCancel)
    document.addEventListener("selectstart", onSelectStart)
    return () => {
      scope.removeEventListener("pointerdown", onPointerDown)
      window.removeEventListener("pointermove", onPointerMove)
      window.removeEventListener("pointerup", onPointerUp)
      window.removeEventListener("pointercancel", onCancel)
      window.removeEventListener("keydown", onKeyDown, true)
      window.removeEventListener("blur", onCancel)
      document.removeEventListener("selectstart", onSelectStart)
      drag?.ghost.remove()
      release()
      tidy()
      announcer.remove()
    }
  }, [store, root])
}
