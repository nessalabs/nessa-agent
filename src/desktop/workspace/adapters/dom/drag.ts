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
 * store is only asked when the zone changes. The preview's motion is marked
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
import { zoneAt } from "../../model/drop"
import type { PaneKey, PaneLayout, Zone } from "../../model/pane-layout"
import { paneLimits } from "../../model/pane-layout"
import { placements, type PanePlacement } from "../../model/pane-sizing"
import { dropSession, movePane, previewDrop } from "../store/commands"
import { durationToken, motionToken } from "./motion"

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

/** The transform that draws an element laid out at `from` at `to`, about its centre. */
function between(from: Box, to: Box): { transform: string; sx: number; sy: number } {
  const sx = to.width / from.width
  const sy = to.height / from.height
  const dx = to.left + to.width / 2 - (from.left + from.width / 2)
  const dy = to.top + to.height / 2 - (from.top + from.height / 2)
  return { transform: `translate(${dx}px, ${dy}px) scale(${sx}, ${sy})`, sx, sy }
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

    let pressed: {
      carried: Carried
      source: HTMLElement
      x: number
      y: number
      pointer: number
    } | null = null
    let drag: {
      carried: Carried
      source: HTMLElement
      /** Where the copy started, and flies back to. */
      home: Box
      grab: { x: number; y: number }
      size: { width: number; height: number }
      ghost: HTMLElement
      /** The copy's content, counter-scaled as its box settles into a place. */
      inner: HTMLElement
      pointer: { x: number; y: number }
      frame: number
      aim: { target: PaneKey; zone: Zone } | null
      outcome: DropOutcome | null
      outline: HTMLElement | null
      /** Where the drop would land, if it would. */
      landing: Box | null
    } | null = null

    const layoutNow = () => store.getState().workspace.panes
    const gridBox = () => {
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
    // Read from the stylesheet when a drag begins, so a frame of the drag never asks for style.
    let motion = { duration: 0, easing: "ease" }
    const timing = () => motion

    // ——— The preview: panes where the drop would put them ———

    const letGo = (element: Element) =>
      element
        .getAnimations()
        .filter((animation) => animation.id === dragPreview)
        .forEach((animation) => {
          animation.cancel()
          previews.get(scope)?.delete(animation)
        })

    /** Draws a pane laid out at `real` at `to`, from wherever it is drawn now. */
    const reflow = (pane: HTMLElement, real: Box, to: Box | null, fade = false) => {
      const now = boxOf(pane.getBoundingClientRect())
      const from = between(real, now)
      const target = to ? between(real, to) : { transform: "none", sx: 1, sy: 1 }
      letGo(pane)
      Array.from(pane.children).forEach(letGo)
      pane.style.transformOrigin = "50% 50%"
      const box = track(
        scope,
        pane.animate(
          [
            { transform: from.transform, opacity: pane.style.opacity || 1 },
            { transform: target.transform, opacity: fade ? 0.2 : 1 },
          ],
          { ...timing(), fill: "forwards", id: dragPreview },
        ),
      )
      // The content keeps its size, as in a flight (`flip.tsx`), held to the
      // pane's top left as it waits there, so it reads from its start.
      Array.from(pane.children, (child) => {
        const element = child as HTMLElement
        element.style.transformOrigin = `0px ${-element.offsetTop}px`
        track(
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
      return box
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

    /** Shows what dropping at `aim` would do, or — with no aim — nothing. */
    const preview = (
      outcome: DropOutcome | null,
      target: PaneKey | null,
      zone: Zone | null,
    ) => {
      const layout = layoutNow()
      const grid = gridBox()
      if (!drag || !layout || !grid) return
      const real = boxes(layout, grid)
      const spare = outcome?.foldSidebar ? foldSpare() : 0
      const landing = outcome ? boxes(outcome.layout, landingGrid(grid, spare)) : null
      scope.toggleAttribute("data-drag-folds", Boolean(outcome?.foldSidebar))
      const still = reducedMotion()
      if (outcome && landing) scope.dataset.dragReflow = ""
      for (const [key, box] of real) {
        const pane = paneElement(key)
        if (!pane) continue
        if (still) continue
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

    /** What the sidebar gives up if the drop folds it. */
    const foldSpare = () => {
      const sidebar = scope.querySelector<HTMLElement>(".workspace-sidebar")
      return sidebar ? sidebar.offsetWidth + paneLimits.gutter : 0
    }

    // ——— Following the pointer ———

    /** The pane and zone under the pointer, from where the panes are laid out, not drawn. */
    const aimAt = (x: number, y: number): { target: PaneKey; zone: Zone } | null => {
      const layout = layoutNow()
      const grid = gridBox()
      if (!layout || !grid) return null
      for (const [key, box] of boxes(layout, grid)) {
        if (
          x < box.left ||
          x > box.left + box.width ||
          y < box.top ||
          y > box.top + box.height
        )
          continue
        return {
          target: key,
          zone: zoneAt((x - box.left) / box.width, (y - box.top) / box.height),
        }
      }
      return null
    }

    const follow = () => {
      if (!drag) return
      drag.frame = 0
      const { pointer } = drag
      const aim = aimAt(pointer.x, pointer.y)
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
     * The copy that is carried, made once when the drag begins and never
     * rendered again: the pane itself — its header, its conversation where it
     * was scrolled to, its composer and chips — or, for a session taken from a
     * list, its heading and conversation as the window holds them, over the
     * focused pane's composer. A picture, not a control: nothing in it takes
     * focus, a pointer, or a name.
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
      const picture = (element: Element): HTMLElement => {
        const clone = element.cloneNode(true) as HTMLElement
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
      const keepScroll = (
        from: Element | null | undefined,
        to: Element | null | undefined,
      ) => {
        if (from && to) to.scrollTop = from.scrollTop
      }
      if (carried.kind === "pane") {
        const pane = paneElement(carried.pane)
        if (pane) {
          const copy = picture(pane)
          copy.removeAttribute("style")
          copy.classList.add("workspace-drag-ghost-pane")
          inner.append(copy)
          scope.append(ghost)
          keepScroll(
            pane.querySelector(".workspace-transcript"),
            copy.querySelector(".workspace-transcript"),
          )
          const typed = pane.querySelectorAll("textarea")
          copy.querySelectorAll("textarea").forEach((field, index) => {
            field.value = typed[index]?.value ?? ""
          })
          return { ghost, inner }
        }
      }
      // A session with no pane yet: its heading and conversation, as the window holds them.
      const card = document.createElement("article")
      card.className = "workspace-pane workspace-drag-ghost-pane"
      const header = document.createElement("header")
      header.className = "workspace-pane-header"
      const tile = source.querySelector(".workspace-agent-tile")
      if (tile) header.append(picture(tile))
      const body = document.createElement("div")
      body.className = "workspace-pane-body"
      const transcript = document.createElement("div")
      transcript.className = "workspace-transcript"
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
      const messages = held?.messages ?? []
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
      scope.append(ghost)
      // Its latest words, as a conversation opens.
      transcript.scrollTop = transcript.scrollHeight
      return { ghost, inner }
    }

    const begin = (event: PointerEvent) => {
      if (!pressed) return
      const { carried, source, x, y } = pressed
      const layout = layoutNow()
      if (!layout) return
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
      const grab = { x: x - home.left, y: y - home.top }
      motion = {
        duration: durationToken(scope, "--desktop-base"),
        easing: motionToken(scope, "--desktop-ease") ?? "ease",
      }
      window.getSelection()?.removeAllRanges()
      const { ghost, inner } = ghostFor(carried, source, size)
      ghost.style.transform = `translate(${home.left}px, ${home.top}px)`
      drag = {
        carried,
        source,
        home,
        grab,
        size,
        ghost,
        inner,
        pointer: { x: event.clientX, y: event.clientY },
        frame: 0,
        aim: null,
        outcome: null,
        outline: null,
        landing: null,
      }
      // With the pointer from its first frame.
      carry()
      scope.dataset.dragging = carried.kind
      source.dataset.dragging = ""
      if (carried.kind === "pane")
        paneElement(carried.pane)?.setAttribute("data-lifted", "")
      try {
        scope.setPointerCapture(pressed.pointer)
      } catch {
        // A pointer the page no longer has: the drag still follows window events.
      }
      schedule()
    }

    /**
     * Ends the drag. What it marked on the page goes at once, or — for a drop,
     * `later` — a frame on, so the commit measures the page as the preview
     * left it rather than laying it out again first.
     */
    const tidy = (later = false) => {
      if (!drag) return
      const { frame, outline, source } = drag
      drag = null
      cancelAnimationFrame(frame)
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

    /** Let go somewhere that is not a zone, or Escape: the copy flies home as the panes go back. */
    const cancel = () => {
      if (!drag) return
      const { ghost, home, size } = drag
      preview(null, null, null)
      const outerNow = getComputedStyle(ghost).transform
      const innerNow = getComputedStyle(drag.inner).transform
      ghost.getAnimations().forEach((animation) => animation.cancel())
      drag.inner.getAnimations().forEach((animation) => animation.cancel())
      const back = ghost.animate(
        [
          { transform: outerNow, opacity: 1 },
          {
            transform: `translate(${home.left}px, ${home.top}px) scale(${home.width / size.width}, ${home.height / size.height})`,
            opacity: 0,
          },
        ],
        { ...timing(), fill: "forwards" },
      )
      drag.inner.animate(
        [
          { transform: innerNow },
          {
            transform: `scale(${size.width / home.width}, ${size.height / home.height})`,
          },
        ],
        { ...timing(), fill: "forwards" },
      )
      const layoutPanes = scope.querySelectorAll<HTMLElement>("[data-pane-key]")
      scope.removeAttribute("data-drag-folds")
      tidy()
      void back.finished
        .catch(() => undefined)
        .then(() => {
          ghost.remove()
          layoutPanes.forEach((pane) => {
            letGo(pane)
            Array.from(pane.children).forEach(letGo)
          })
          // Back where they were: blur and shadows return, unless a drag began meanwhile.
          if (!drag) delete scope.dataset.dragReflow
        })
    }

    /** Let go over a zone: the command commits what was previewed, and the copy hands over. */
    const drop = () => {
      if (!drag) return
      const { carried, aim, outcome, ghost, landing } = drag
      if (!aim || !outcome) return cancel()
      // Now it snaps: the copy flies from the pointer into the place it takes.
      const flight = landing ? settleGhost(landing) : null
      tidy(true)
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
      // Whatever `FlipScope` did not measure through — a drop that changed no
      // arrangement — lets go a frame on.
      requestAnimationFrame(() =>
        requestAnimationFrame(() => {
          // The side columns' preview of a fold, measured through by the commit, goes now.
          scope.removeAttribute("data-drag-folds")
          letGoOfDragPreview(scope)
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
      pressed = {
        carried,
        source,
        x: event.clientX,
        y: event.clientY,
        pointer: event.pointerId,
      }
    }

    const onPointerMove = (event: PointerEvent) => {
      if (drag) {
        drag.pointer = { x: event.clientX, y: event.clientY }
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

    const swallowClick = (event: MouseEvent) => {
      event.stopPropagation()
      event.preventDefault()
    }

    const onPointerUp = () => {
      pressed = null
      if (!drag) return
      // The press that ended a drag is not also a click on what it started from.
      window.addEventListener("click", swallowClick, { capture: true, once: true })
      window.setTimeout(() => window.removeEventListener("click", swallowClick, true), 0)
      drop()
    }

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !drag) return
      event.preventDefault()
      event.stopPropagation()
      pressed = null
      cancel()
    }

    const onCancel = () => {
      pressed = null
      cancel()
    }

    scope.addEventListener("pointerdown", onPointerDown)
    window.addEventListener("pointermove", onPointerMove)
    window.addEventListener("pointerup", onPointerUp)
    window.addEventListener("pointercancel", onCancel)
    window.addEventListener("keydown", onKeyDown, true)
    window.addEventListener("blur", onCancel)
    return () => {
      scope.removeEventListener("pointerdown", onPointerDown)
      window.removeEventListener("pointermove", onPointerMove)
      window.removeEventListener("pointerup", onPointerUp)
      window.removeEventListener("pointercancel", onCancel)
      window.removeEventListener("keydown", onKeyDown, true)
      window.removeEventListener("blur", onCancel)
      drag?.ghost.remove()
      tidy()
      announcer.remove()
    }
  }, [store, root])
}
