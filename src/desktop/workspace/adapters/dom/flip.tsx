/**
 * FLIP for the workspace: a layout change lands at once, and what moved plays
 * back from where it was by transform alone, so no frame of the motion lays
 * out text again.
 *
 * `FlipScope` measures in React's commit phase, just before the DOM changes
 * (`getSnapshotBeforeUpdate`), and plays just after. It therefore animates any
 * change that reaches the store — a click, a keystroke, or an agent's
 * dispatch — with no hook wrapped around each command. It measures only
 * when `shape` changes, so dragging an edge (a change of size, not of shape)
 * never animates.
 *
 * What moves is marked in the markup:
 *
 * - `data-flip="pane"` with `data-flip-id`: travels and resizes, its contents
 *   counter-scaled so text keeps its size (see `flyPane`);
 * - `data-flip="slide"` with `data-flip-id`: slides sideways. A slide inside
 *   another slide moves only by its own difference.
 *
 * While anything flies, the scope's root carries `data-flipping`, which the
 * stylesheet uses to pause blur and large shadows.
 */
import { Component, type ReactNode, type RefObject } from "react"
import { letGoOfDragPreview } from "./drag"
import { durationToken, motionToken } from "./motion"

type Rects = { panes: Map<string, DOMRect>; slides: Map<string, DOMRect> }

function measure(root: HTMLElement): Rects {
  const rects: Rects = { panes: new Map(), slides: new Map() }
  root.querySelectorAll<HTMLElement>("[data-flip][data-flip-id]").forEach((element) => {
    const id = element.dataset.flipId ?? ""
    const map = element.dataset.flip === "pane" ? rects.panes : rects.slides
    map.set(id, element.getBoundingClientRect())
  })
  return rects
}

const moved = (a: DOMRect, b: DOMRect) =>
  Math.abs(a.left - b.left) >= 0.5 ||
  Math.abs(a.top - b.top) >= 0.5 ||
  Math.abs(a.width - b.width) >= 0.5 ||
  Math.abs(a.height - b.height) >= 0.5

/**
 * FLIP with scale correction: the pane's box travels and resizes from `from`
 * to `to` by transform, while its children counter-scale on the same curve,
 * so the text, laid out once at its final size, never stretches. Both scale
 * about the pane's centre, so what is centred in it glides from its old
 * centre to its new one.
 */
export function flyPane(pane: HTMLElement, from: DOMRect, to: DOMRect): Animation[] {
  const steps = 14
  const ease = (t: number) => 1 - Math.pow(1 - t, 3.2)
  const dx = from.left + from.width / 2 - (to.left + to.width / 2)
  const dy = from.top + from.height / 2 - (to.top + to.height / 2)
  const sx = from.width / to.width
  const sy = from.height / to.height
  const box: Keyframe[] = []
  const inner: Keyframe[] = []
  for (let i = 0; i <= steps; i++) {
    const e = ease(i / steps)
    const x = sx + (1 - sx) * e
    const y = sy + (1 - sy) * e
    box.push({
      transform: `translate(${dx * (1 - e)}px, ${dy * (1 - e)}px) scale(${x}, ${y})`,
    })
    inner.push({ transform: `scale(${1 / x}, ${1 / y})` })
  }
  const timing: KeyframeAnimationOptions = {
    duration: durationToken(pane, "--desktop-flight"),
    easing: "linear",
  }
  pane.style.transformOrigin = "50% 50%"
  // A pane that changes size lays its header out at the size it lands at,
  // so mid-flight its controls would float inside the box, or be cut off at
  // its edge; it waits out of sight and comes back as the pane lands.
  const resized = Math.abs(sx - 1) > 0.02 || Math.abs(sy - 1) > 0.02
  const header = pane.querySelector<HTMLElement>(":scope > .workspace-pane-header")
  const hiding =
    resized && header
      ? [
          header.animate([{ opacity: 0 }, { opacity: 0, offset: 0.7 }, { opacity: 1 }], {
            duration: durationToken(pane, "--desktop-flight"),
            easing: "linear",
          }),
        ]
      : []
  return [
    ...hiding,
    pane.animate(box, timing),
    ...Array.from(pane.children, (child) => {
      const element = child as HTMLElement
      // The pane's centre, in the child's own box.
      element.style.transformOrigin = `${to.width / 2}px ${to.height / 2 - element.offsetTop}px`
      return element.animate(inner, timing)
    }),
  ]
}

function play(root: HTMLElement, from: Rects): Animation[] {
  const flying: Animation[] = []
  const shifted = new Map<HTMLElement, number>()
  const ease = motionToken(root, "--desktop-ease") ?? "linear"
  const duration = durationToken(root, "--desktop-slow")
  root
    .querySelectorAll<HTMLElement>('[data-flip="slide"][data-flip-id]')
    .forEach((element) => {
      const before = from.slides.get(element.dataset.flipId ?? "")
      // Only what is on screen slides; a column folding away has its own transition.
      if (!before || element.closest("[inert]")) return
      const outer = element.parentElement?.closest<HTMLElement>('[data-flip="slide"]')
      const carried = (outer && shifted.get(outer)) ?? 0
      const dx = before.left - element.getBoundingClientRect().left
      shifted.set(element, dx)
      const own = dx - carried
      if (Math.abs(own) < 0.5) return
      flying.push(
        element.animate([{ transform: `translateX(${own}px)` }, { transform: "none" }], {
          duration,
          easing: ease,
        }),
      )
    })
  root
    .querySelectorAll<HTMLElement>('[data-flip="pane"][data-flip-id]')
    .forEach((pane) => {
      const before = from.panes.get(pane.dataset.flipId ?? "")
      if (!before) return
      const after = pane.getBoundingClientRect()
      if (moved(before, after)) flying.push(...flyPane(pane, before, after))
    })
  return flying
}

export class FlipScope extends Component<{
  /** What the layout looks like, ignoring sizes; motion plays when it changes. */
  shape: string
  root: RefObject<HTMLElement | null>
  children: ReactNode
}> {
  private flights: Animation[] = []

  getSnapshotBeforeUpdate(previous: Readonly<{ shape: string }>): Rects | null {
    const root = this.props.root.current
    if (previous.shape === this.props.shape || !root) return null
    // Reduced motion makes the flight's duration zero: nothing to measure for.
    if (durationToken(root, "--desktop-flight") === 0) return null
    // Read mid-flight too: a change during a flight starts from where things are now.
    return measure(root)
  }

  componentDidUpdate(_previous: unknown, _state: unknown, snapshot: Rects | null) {
    const root = this.props.root.current
    if (!snapshot || !root) return
    this.flights.forEach((flight) => flight.cancel())
    // A drag's preview put panes where this change puts them: measured through
    // it before the change, it is let go before measuring where they landed,
    // so a drop that lands where it previewed has nowhere to fly.
    letGoOfDragPreview(root)
    const flying = play(root, snapshot)
    this.flights = flying
    if (flying.length === 0) {
      delete root.dataset.flipping
      return
    }
    root.dataset.flipping = ""
    Promise.all(flying.map((flight) => flight.finished))
      .then(() => {
        if (this.flights === flying) delete root.dataset.flipping
      })
      .catch(() => undefined)
  }

  componentWillUnmount() {
    this.flights.forEach((flight) => flight.cancel())
  }

  render() {
    return this.props.children
  }
}
