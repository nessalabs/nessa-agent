/**
 * Drag and drop in the workspace, carried by the pointer rather than the
 * browser's own drag, so what is carried looks like the window it will be.
 * What a press becomes, event by event, is `model/drag.ts` (ADR 238 › _Drag
 * and drop_ has its table); this is the page's side of it — every event is
 * sent there, and each phase it answers with is drawn here:
 *
 * - **pressed**: nothing in the press's own frame; in a task once it has
 *   painted, the copy is made unseen — the pane itself, or for a session from
 *   a list its heading and latest words — and what the drag needs of the page
 *   is read, once: the grid and its room, each pane's box and parts, the side
 *   columns drawn. Until then a move does not lift it;
 * - **carrying**: the copy is shown under the pointer and glides so its
 *   centre comes under it, then stays there; the zone the pointer is in
 *   (`aimAt`) shows what dropping there would do — the panes move where the
 *   drop would put them and a placeholder marks the rect it takes, from the
 *   one outcome the command commits (`previewDrop`);
 * - **dropping**: released in the zone previewed, the command commits that
 *   outcome in the room the press read, and the copy flies into the
 *   placeholder's rect and hands over to the real pane;
 * - **cancelling**: the copy flies home as the panes go back — or, when the
 *   room, the panes or the view changed under it, both go at once and the
 *   change plays as it would with no drag.
 *
 * Everything moves by transform, in animations the compositor runs; the
 * pointer is followed without rendering anything, and the store is asked
 * only when the zone changes. No frame of a drag makes the page lay out
 * early: after the press's frame nothing is read again — a change that would
 * make what was read wrong ends the drag instead. Where a preview has drawn a
 * pane is known from the preview's own motion, never read back. The
 * preview's motion is marked (`dragPreview`) so `FlipScope` measures through
 * it at the drop and lets it go before it measures where things landed. With
 * less motion, nothing moves: the copy follows the pointer and the place it
 * would land is outlined. The zone is said to assistive technology as it
 * changes.
 *
 * A component takes part by marking what can be carried: `data-drag-pane`
 * (a pane's key, on its header) or `data-drag-session` (a session's id, on a
 * row). The keyboard's own ways — the Move items, ⌃⌥ and an arrow — are
 * untouched.
 */
import { useEffect, type RefObject } from "react"
import { reducedMotion } from "../../../adapters/motion-preference"
import type { DesktopStore } from "../../../store"
import {
  idle,
  keyToDrag,
  stepDrag,
  type DragEvent,
  type DragPhase,
} from "../../model/drag"
import {
  restAfter,
  type Aim,
  type Carried,
  type DropOutcome,
  type PointerSample,
  type Targets,
} from "../../model/drop"
import type { PaneKey, PaneLayout, Zone } from "../../model/pane-layout"
import { paneLimits } from "../../model/pane-layout"
import {
  placements,
  type PanePlacement,
  type WorkspaceRoom,
} from "../../model/pane-sizing"
import { dropSession, measureRoom, movePane, previewDrop } from "../store/commands"
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
  const held = previews.get(root)
  if (!held) return
  held.forEach((animation) => animation.cancel())
  previews.delete(root)
}

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

const sameAim = (a: Aim | null, b: Aim | null) =>
  a?.target === b?.target && a?.zone === b?.zone

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
  /** The copy, drawn about the pointer: its offset glides to its centre. */
  readonly ghost: HTMLElement
  /** The copy's content, counter-scaled as its box settles into a place. */
  readonly inner: HTMLElement
  /** What was pressed: marked while carried. */
  readonly source: HTMLElement
  /** Over the page while carrying: the grabbing hand, and nothing under it selected. */
  readonly shield: HTMLElement
  /** Where the copy started, and flies back to. */
  readonly home: Box
  /** Where the pointer holds the copy as it is lifted, from its top left. */
  readonly grab: { x: number; y: number }
  readonly size: { width: number; height: number }
  readonly grid: Box
  /** The panes' room as the press found it: what the preview and the drop are held to. */
  readonly room: WorkspaceRoom | undefined
  /** What the drag may aim at; `null` while no pane can be seen. */
  readonly targets: Targets | null
  /** Each pane's parts and where each sits in it: what a preview counter-scales. */
  readonly parts: ReadonlyMap<
    HTMLElement,
    readonly { element: HTMLElement; top: number }[]
  >
  readonly motion: { duration: number; easing: string }
  /** What is drawn now, written as the drag goes. */
  readonly drawing: {
    pointer: { x: number; y: number }
    /** The copy's glide, from where it was grabbed to its centre under the pointer. */
    glide: Animation | null
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

    /** The phase of the one drag (`model/drag.ts`), holding what the page made for it. */
    let phase: DragPhase<Made> = idle
    /** What is being carried now, if anything. */
    const carried = () => (phase.kind === "carrying" ? phase.made : null)

    /** The press's frame, then the task after it, that make what a drag needs. */
    let waiting: { frame: number; timer: number } | null = null

    /** The workspace as the press found it: a change to any of it ends the press or the drag. */
    let seen: { panes: unknown; content: unknown; chrome: unknown } = {
      panes: null,
      content: null,
      chrome: null,
    }

    const layoutNow = () => store.getState().workspace.panes
    const paneElement = (key: PaneKey) =>
      scope.querySelector<HTMLElement>(`[data-pane-key="${key}"]`)

    // ——— The preview: panes where the drop would put them ———

    // Each pane's preview, as it was asked for: where it is drawn is where
    // this says, part way on its clock — never read back from the page.
    const previewed = new Map<
      HTMLElement,
      { from: Drawn; to: Drawn; motion: Animation; parts: Animation[] }
    >()
    const drawnNow = (pane: HTMLElement): Drawn => {
      const held = previewed.get(pane)
      if (!held) return inPlace
      return partWay(held.from, held.to, progressOf(held.motion))
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
    const reflow = (
      made: Made,
      pane: HTMLElement,
      real: Box,
      to: Box | null,
      fade = false,
    ) => {
      const from = drawnNow(pane)
      const target = to ? between(real, to, fade ? 0.2 : 1) : inPlace
      letGo(pane)
      const parts = made.parts.get(pane) ?? []
      pane.style.transformOrigin = "50% 50%"
      const motion = track(
        scope,
        pane.animate(
          [
            { transform: transformOf(from), opacity: from.opacity },
            { transform: transformOf(target), opacity: target.opacity },
          ],
          { ...made.motion, fill: "forwards", id: dragPreview },
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
            { ...made.motion, fill: "forwards", id: dragPreview },
          ),
        )
      })
      previewed.set(pane, { from, to: target, motion, parts: counter })
    }

    /**
     * The calm placeholder where the drop would land: a soft fill and a
     * hairline in the theme's edge light, at exactly the rect the pane will
     * take. It moves between zones on its own short transition.
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
      element.className = "workspace-drag-placeholder"
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
      const spare = outcome?.foldSidebar ? (room?.spare ?? 0) : 0
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
            made,
            pane,
            box,
            outcome && to ? inside(to, landingGrid(grid, spare)) : null,
            replaced,
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

    /** Brings what the page shows up to the phase's aim: asked of the store only when it changed. */
    const show = () => {
      if (phase.kind !== "carrying") return
      const { aim, carried: what, made } = phase
      const { drawing } = made
      drawing.frame = 0
      if (sameAim(aim, drawing.shown)) return
      drawing.shown = aim
      // A pane over itself, or a zone the room refuses: nothing is offered.
      const outcome = aim
        ? store.dispatch(
            previewDrop({
              carried: what,
              target: aim.target,
              zone: aim.zone,
              room: made.room,
            }),
          )
        : null
      if (outcome === drawing.outcome) return
      drawing.outcome = outcome
      preview(made, outcome, aim)
    }

    // ——— The copy ———

    /** The copy's offset from the pointer: where it was grabbed, gliding to its centre. */
    const offsetNow = ({ grab, size, drawing }: Made) => {
      const progress = progressOf(drawing.glide)
      return {
        x: grab.x + (size.width / 2 - grab.x) * progress,
        y: grab.y + (size.height / 2 - grab.y) * progress,
      }
    }

    /**
     * The copy flies into `box` — onto its place, or home — by transform
     * alone, from where it is drawn now. Its box is scaled and its content
     * counter-scaled, so its words keep their size and nothing is laid out
     * again, as a pane's flight does (`flip.tsx`).
     */
    const flyTo = (made: Made, box: Box, fade: boolean) => {
      const { ghost, inner, size, drawing } = made
      const offset = offsetNow(made)
      drawing.glide?.cancel()
      const sx = box.width / size.width
      const sy = box.height / size.height
      const options: KeyframeAnimationOptions = { ...made.motion, fill: "forwards" }
      inner.animate(
        [{ transform: "none" }, { transform: `scale(${1 / sx}, ${1 / sy})` }],
        options,
      )
      return ghost.animate(
        [
          { transform: `translate(${-offset.x}px, ${-offset.y}px)`, opacity: 1 },
          {
            transform: `translate(${box.left - drawing.pointer.x}px, ${box.top - drawing.pointer.y}px) scale(${sx}, ${sy})`,
            opacity: fade ? 0 : 1,
          },
        ],
        options,
      )
    }

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
     * Everything a drag will need, read and made in a task after the press's
     * frame has painted — the copy put on the page unseen. The page is read
     * here and nowhere after: the grid and the room it measures
     * (`measureRoom`), each pane's box and parts, and the side columns drawn.
     * A page something wrote to since that frame is laid out by these reads,
     * once (ADR 238 › _Drag and drop_ has the measured cost). A press that
     * never becomes a drag takes it all away again.
     */
    const prepare = (
      carried: Carried,
      source: HTMLElement,
      x: number,
      y: number,
    ): Made | null => {
      const layout = layoutNow()
      const panes = scope.querySelector<HTMLElement>(".workspace-panes")
      const room = store.dispatch(measureRoom())
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
      // The side columns drawn — docked, or revealed from the edge over the
      // panes — are never targets.
      const { sidebar, list } = scope.dataset
      const covered = [
        sidebar === "open" || "peek" in scope.dataset
          ? scope.querySelector(".workspace-sidebar")
          : null,
        list === "open" ? scope.querySelector(".workspace-list") : null,
      ].flatMap((column) => (column ? [boxOf(column.getBoundingClientRect())] : []))
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
      // Only panes a person can see are aimed at: none under the Agents overview.
      const targets: Targets | null =
        store.getState().workspace.content === "panes"
          ? { grid, panes: [...boxes(layout, grid)], covered }
          : null
      const { ghost, inner } = ghostFor(carried, source, size)
      const grab = { x: x - home.left, y: y - home.top }
      const layer = document.createElement("div")
      layer.className = "workspace-drag-layer"
      layer.setAttribute("aria-hidden", "true")
      const carrier = document.createElement("div")
      carrier.className = "workspace-drag-carrier"
      carrier.style.transform = `translate(${x}px, ${y}px)`
      ghost.style.transform = `translate(${-grab.x}px, ${-grab.y}px)`
      // On the page, unseen, until the press becomes a drag.
      ghost.dataset.waiting = ""
      carrier.append(ghost)
      layer.append(carrier)
      scope.append(layer)
      const shield = document.createElement("div")
      shield.className = "workspace-drag-shield"
      shield.setAttribute("aria-hidden", "true")
      return {
        layer,
        carrier,
        ghost,
        inner,
        source,
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
      const { carrier, ghost, grab, size, shield, source, drawing } = made
      drawing.pointer = { x: at.x, y: at.y }
      shield.addEventListener("lostpointercapture", onLost)
      window.getSelection()?.removeAllRanges()
      // With the pointer from its first frame, and seen from it.
      carrier.style.transform = `translate(${at.x}px, ${at.y}px)`
      delete ghost.dataset.waiting
      drawing.glide = ghost.animate(
        [
          { transform: `translate(${-grab.x}px, ${-grab.y}px)` },
          { transform: `translate(${-size.width / 2}px, ${-size.height / 2}px)` },
        ],
        { ...made.motion, fill: "forwards" },
      )
      scope.append(shield)
      scope.dataset.dragging = what.kind
      source.dataset.dragging = ""
      if (what.kind === "pane") paneElement(what.pane)?.setAttribute("data-lifted", "")
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
      const { drawing, source, shield } = made
      cancelAnimationFrame(drawing.frame)
      window.clearTimeout(drawing.still)
      shield.remove()
      window.getSelection()?.removeAllRanges()
      const { outline } = drawing
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

    /** The copy's last flight ended: the drag is over. */
    const landed = (made: Made) => {
      made.layer.remove()
      send({ kind: "landed" })
    }

    /**
     * Cancelled. Nothing around it changed (`home`): the copy flies home as
     * the panes go back, from where the pointer left it — known, not read.
     * The room, the panes or the view changed (`at-once`): the copy and the
     * preview go now, and the change plays as it would with no drag.
     */
    const cancel = (made: Made, how: "home" | "at-once") => {
      if (how === "at-once") {
        made.drawing.glide?.cancel()
        ;[...previewed.keys()].forEach(letGo)
        scope.removeAttribute("data-drag-folds")
        tidy(made)
        landed(made)
        return
      }
      preview(made, null, null)
      const back = flyTo(made, made.home, true)
      const panes = [...previewed.keys()]
      scope.removeAttribute("data-drag-folds")
      tidy(made)
      void back.finished
        .catch(() => undefined)
        .then(() => {
          panes.forEach(letGo)
          // Back where they were: blur and shadows return, unless a drag began meanwhile.
          if (!carried()) delete scope.dataset.dragReflow
          landed(made)
        })
    }

    /** Let go in the zone previewed: the command commits what was previewed, and the copy hands over. */
    const drop = (made: Made, what: Carried, aim: Aim) => {
      const { ghost, room, drawing } = made
      // The command first, in the room the preview was drawn for — nothing of
      // the page is read — then the copy's flight and the tidying.
      if (what.kind === "pane")
        store.dispatch(
          movePane({ pane: what.pane, target: aim.target, zone: aim.zone, room }),
        )
      else
        store.dispatch(
          dropSession({
            sessionId: what.sessionId,
            target: aim.target,
            zone: aim.zone,
            room,
          }),
        )
      // Now it snaps: the copy flies from the pointer into the place it takes.
      const flight = drawing.landing ? flyTo(made, drawing.landing, false) : null
      tidy(made, true)
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
        .then(() => landed(made))
    }

    /** A press no longer waiting to become a drag: its pending frame and task go. */
    const stopWaiting = () => {
      if (!waiting) return
      cancelAnimationFrame(waiting.frame)
      window.clearTimeout(waiting.timer)
      waiting = null
    }

    // The window going inert under Settings is seen as it happens, not at the next move.
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
        inert.disconnect()
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
      else if (was.kind === "carrying" && next.kind === "cancelling")
        cancel(was.made, next.how)
    }

    const sample = (event: PointerEvent): PointerSample => ({
      x: event.clientX,
      y: event.clientY,
      t: event.timeStamp,
    })

    /** Whether what the drag read still holds: the panes, the view and the side columns as the press found them. */
    const unchanged = () => {
      if (phase.kind !== "pressed" && phase.kind !== "carrying") return true
      const { panes, content, chrome, sessions } = store.getState().workspace
      // The panes' arrangement, not which has focus: pressing a pane focuses it.
      if (
        panes?.columns !== seen.panes ||
        content !== seen.content ||
        chrome !== seen.chrome
      )
        return false
      const { carried: what } = phase
      return what.kind !== "session" || Object.hasOwn(sessions, what.sessionId)
    }

    const onPointerDown = (event: PointerEvent) => {
      if (phase.kind === "pressed" || phase.kind === "carrying") {
        // Another button of the pointer that carries: the press or the drag ends.
        if (event.pointerId === phase.pointerId) send({ kind: "lost" })
        return
      }
      if (event.button !== 0 || phase.kind !== "idle") return
      const target = event.target as Element
      const source = target.closest<HTMLElement>("[data-drag-pane], [data-drag-session]")
      if (!source || !scope.contains(source)) return
      // A control inside what is carried keeps its own press.
      const control = target.closest("button, input, textarea, a, [role='menuitem']")
      if (control && control !== source && source.contains(control)) return
      const pane = source.dataset.dragPane
      const session = source.dataset.dragSession
      const what: Carried | null =
        pane !== undefined
          ? { kind: "pane", pane: Number(pane) }
          : session
            ? { kind: "session", sessionId: session }
            : null
      if (!what) return
      const { panes, content, chrome } = store.getState().workspace
      seen = { panes: panes?.columns, content, chrome }
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
          const made = prepare(what, source, x, y)
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
          previewed: null,
        })
        return
      }
      if (phase.kind !== "carrying" || event.pointerId !== phase.pointerId) return
      // The press that ended a drag is not also a click on what it started from.
      window.addEventListener("click", swallowClick, { capture: true, once: true })
      window.setTimeout(() => window.removeEventListener("click", swallowClick, true), 0)
      const { targets, drawing } = phase.made
      send({
        kind: "release",
        pointerId: event.pointerId,
        at: sample(event),
        targets,
        // Only what the page has shown may be committed.
        previewed: drawing.outcome ? drawing.shown : null,
      })
    }

    // A pointer the page loses hold of — taken by the system, or gone with
    // the window — ends the press or the drag as Escape does, and a later
    // move starts nothing.
    const onLost = () => send({ kind: "lost" })

    const onKeyDown = (event: KeyboardEvent) => {
      if (phase.kind !== "pressed" && phase.kind !== "carrying") return
      // Under Settings the keys are Settings' own, its Escape included.
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
    const unsubscribe = store.subscribe(onStore)

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
      if (phase.kind === "pressed" || phase.kind === "carrying") {
        phase.made?.layer.remove()
        if (phase.made) tidy(phase.made)
      }
      phase = idle
      announcer.remove()
    }
  }, [store, root])
}
