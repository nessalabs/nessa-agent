/**
 * The overview's list re-flowing, played back by transform: when a request
 * leaves or a session changes group, the page lays out at once and each item
 * travels from where it was drawn to where it now is (FLIP); an item that was
 * not there before materializes where it lands, in the same beat — so a
 * session moving group leaves one place (`leaveInPlace`) as it arrives in
 * the other, as if it went straight there. Only transform, opacity and a
 * light blur move; nothing lays text out again.
 *
 * Where each item was is kept as laid out within the list (which must be
 * positioned), so scrolling moves nothing, and taken again whenever the
 * list's size changes without a re-flow — a request read in, a window
 * resized — so a later re-flow starts from what was really on screen.
 *
 * Only a change of `shape` — what the list holds, and in what order — is a
 * re-flow, and only then is the page read after it changes: a render that
 * moves nothing (a row chosen, an answer on its way) reads nothing, so it
 * never makes the page lay out early. Durations are the window's tokens,
 * which less motion sets to zero; with zero there is nothing to play.
 */
import { useLayoutEffect, useRef, type RefObject } from "react"
import { durationToken, motionToken } from "../../../adapters/motion"

const itemSelector = "[data-reflow]"

type Places = Map<string, { top: number; left: number }>

/**
 * Where an item is laid out within the list — never where a flight draws it,
 * so a re-flow asked for mid-flight starts from the layout, not a transform.
 */
function offsetWithin(
  item: HTMLElement,
  list: HTMLElement,
): { top: number; left: number } {
  let top = 0
  let left = 0
  let at: HTMLElement | null = item
  while (at && at !== list) {
    top += at.offsetTop
    left += at.offsetLeft
    const parent = at.offsetParent as HTMLElement | null
    // The list is positioned, so every chain ends at it; one that does not is measured from the page.
    if (parent && !list.contains(parent) && parent !== list) break
    at = parent
  }
  return { top, left }
}

function places(list: HTMLElement): Places {
  const found: Places = new Map()
  for (const item of list.querySelectorAll<HTMLElement>(itemSelector)) {
    const key = item.dataset.reflow
    if (key) found.set(key, offsetWithin(item, list))
  }
  return found
}

/**
 * Plays the list's re-flow whenever its `shape` changes. Items carry
 * `data-reflow` with a key of their own among the list.
 */
export function useReflow(list: RefObject<HTMLElement | null>, shape: string): void {
  const before = useRef<Places | null>(null)

  useLayoutEffect(() => {
    const element = list.current
    // The first layout is the page arriving, which has its own motion: it is
    // not read here, where reading would lay the page out early — the
    // observer below takes it once the page has laid out on its own.
    const was = before.current
    if (!element || !was) return
    const now = places(element)
    before.current = now
    if (typeof element.animate !== "function") return
    const duration = durationToken(element, "--desktop-slow")
    if (duration === 0) return
    const easing = motionToken(element, "--desktop-out") ?? "ease-out"
    dematerialize(element)
    for (const item of element.querySelectorAll<HTMLElement>(itemSelector)) {
      const key = item.dataset.reflow
      if (!key) continue
      const to = now.get(key)
      const from = was.get(key)
      if (!to) continue
      if (!from) {
        // Materializes as whatever left for it dematerializes, from the same frame.
        item.animate(
          [
            { opacity: 0, transform: "scale(0.98)", filter: "blur(4px)" },
            { opacity: 1, transform: "none", filter: "blur(0px)" },
          ],
          { duration, easing, fill: "backwards" },
        )
        continue
      }
      const dy = from.top - to.top
      const dx = from.left - to.left
      if (Math.abs(dy) < 0.5 && Math.abs(dx) < 0.5) continue
      item.animate(
        [{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }],
        { duration, easing },
      )
    }
  }, [list, shape])

  // Sizes that change without a render still move what follows them.
  useLayoutEffect(() => {
    const element = list.current
    if (!element || typeof ResizeObserver === "undefined") return
    const observer = new ResizeObserver(() => {
      before.current = places(element)
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [list])
}

/** Copies of items taken away, left where they were until the re-flow plays them out. */
const departing = new WeakMap<HTMLElement, HTMLElement[]>()

/** Plays out the list's departing copies, from the frame the re-flow starts. */
function dematerialize(list: HTMLElement): void {
  const ghosts = departing.get(list) ?? []
  departing.delete(list)
  const duration = durationToken(list, "--desktop-slow")
  const easing = motionToken(list, "--desktop-ease") ?? "ease"
  for (const ghost of ghosts) {
    if (duration === 0 || typeof ghost.animate !== "function") {
      ghost.remove()
      continue
    }
    const leaving = ghost.animate(
      [
        { opacity: 1, transform: "none", filter: "blur(0px)" },
        { opacity: 0, transform: "scale(0.97)", filter: "blur(4px)" },
      ],
      { duration, easing, fill: "forwards" },
    )
    leaving.finished.then(
      () => ghost.remove(),
      () => ghost.remove(),
    )
  }
}

/**
 * An item about to be taken away leaves a copy of itself where it is: drawn
 * over it, the same to the eye, out of reach of the keyboard and screen
 * readers, and out of the re-flow (no `data-reflow`). The re-flow that the
 * item's going causes plays the copy out — fading, a little smaller, a
 * little blurred — in the same frame as the items after it close the gap and
 * as the session's row arrives in its new group. Called just before the
 * change that takes the item away. With less motion, or no re-flow to play
 * it within two frames, the copy simply goes.
 */
export function leaveInPlace(list: HTMLElement, item: HTMLElement): void {
  if (typeof item.animate !== "function") return
  if (durationToken(list, "--desktop-slow") === 0) return
  const { top, left } = offsetWithin(item, list)
  const ghost = item.cloneNode(true) as HTMLElement
  for (const marked of [ghost, ...ghost.querySelectorAll<HTMLElement>("*")]) {
    marked.removeAttribute("data-reflow")
    marked.removeAttribute("data-overview-item")
    marked.removeAttribute("id")
    marked.removeAttribute("tabindex")
  }
  // Its corners as drawn where it stood (first, last or between), not as the
  // list's last child it now is.
  const drawn = item.firstElementChild
  const copy = ghost.firstElementChild
  if (drawn instanceof HTMLElement && copy instanceof HTMLElement)
    copy.style.borderRadius = getComputedStyle(drawn).borderRadius
  ghost.setAttribute("aria-hidden", "true")
  ghost.inert = true
  ghost.dataset.departing = ""
  Object.assign(ghost.style, {
    position: "absolute",
    top: `${top}px`,
    left: `${left}px`,
    width: `${item.offsetWidth}px`,
    height: `${item.offsetHeight}px`,
    margin: "0",
    pointerEvents: "none",
    listStyle: "none",
  })
  list.append(ghost)
  departing.set(list, [...(departing.get(list) ?? []), ghost])
  // A change that turns out to move nothing (the session kept where it was)
  // never re-flows: the copy goes on its own a frame later.
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      if (departing.get(list)?.includes(ghost)) dematerialize(list)
    }),
  )
}
