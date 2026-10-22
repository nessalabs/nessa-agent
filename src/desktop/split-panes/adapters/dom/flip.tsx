/**
 * FLIP for a host's panes and columns: a layout change lands at once, and what moved plays
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
 * The scope marks an active flight and each pane it moves. Styles pause the
 * moving panes' blur and shadows; stationary panes stay live.
 */
import { Component, type ReactNode, type RefObject } from "react"
import { slideAnimation } from "../../../adapters/hold-still"
import { letGoOfDragPreview } from "./drag"
import { marks, reflectMark } from "./marks"
import { durationToken, motionToken } from "../../../adapters/motion"

type Rects = { panes: Map<string, DOMRect>; slides: Map<string, DOMRect> }
type Snapshot = { readonly root: HTMLElement; readonly rects: Rects }

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

/** A pane where it landed, read with everything else before any flight starts. */
interface Landed {
  readonly pane: HTMLElement
  readonly to: DOMRect
  /** Each child and where it sits in the pane. */
  readonly children: readonly { element: HTMLElement; top: number }[]
}

/**
 * FLIP with scale correction: the pane's box travels and resizes from `from`
 * to `to` by transform, while its children counter-scale on the same curve,
 * so the text, laid out once at its final size, never stretches. Both scale
 * about the pane's centre, so what is centred in it glides from its old
 * centre to its new one. It only writes: what it needs of the page was read
 * beforehand (`Landed`), so flights begun one after another never make the
 * page lay out again between them.
 */
export function flyPane(
  { pane, to, children }: Landed,
  from: DOMRect,
  duration: number,
): Animation[] {
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
  const timing: KeyframeAnimationOptions = { duration, easing: "linear" }
  pane.style.transformOrigin = "50% 50%"
  return [
    pane.animate(box, timing),
    ...children.map(({ element, top }) => {
      // The pane's centre, in the child's own box.
      element.style.transformOrigin = `${to.width / 2}px ${to.height / 2 - top}px`
      return element.animate(inner, timing)
    }),
  ]
}

function play(root: HTMLElement, from: Rects) {
  const flying: Animation[] = []
  const shifted = new Map<HTMLElement, number>()
  // Where everything landed, read before anything starts: a slide begun first
  // would be read back into the place of what it carries, and every write
  // between two reads lays the page out again.
  const landed = [
    ...root.querySelectorAll<HTMLElement>('[data-flip="slide"][data-flip-id]'),
  ].map((element) => ({ element, after: element.getBoundingClientRect() }))
  const panes: { landed: Landed; before: DOMRect }[] = []
  for (const pane of root.querySelectorAll<HTMLElement>(
    '[data-flip="pane"][data-flip-id]',
  )) {
    const to = pane.getBoundingClientRect()
    const before = from.panes.get(pane.dataset.flipId ?? "")
    // A settled drop has already previewed where these panes land. Its
    // stationary contents need no counter-scale offsets (flip.test.tsx).
    if (!before || !moved(before, to)) continue
    panes.push({
      before,
      landed: {
        pane,
        to,
        children: Array.from(pane.children, (child) => ({
          element: child as HTMLElement,
          top: (child as HTMLElement).offsetTop,
        })),
      },
    })
  }
  const slides: { element: HTMLElement; own: number }[] = []
  landed.forEach(({ element, after }) => {
    const before = from.slides.get(element.dataset.flipId ?? "")
    // Only what is on screen slides; a column folding away has its own transition.
    if (!before || element.closest("[inert]")) return
    const outer = element.parentElement?.closest<HTMLElement>('[data-flip="slide"]')
    const carried = (outer && shifted.get(outer)) ?? 0
    const dx = before.left - after.left
    shifted.set(element, dx)
    const own = dx - carried
    if (Math.abs(own) < 0.5) return
    slides.push({ element, own })
  })
  // A settled preview can leave nothing to animate. Resolving root styles
  // then does work for no flight (flip.test.tsx holds this no-flight path).
  const moving = panes.map(({ landed }) => landed.pane)
  if (slides.length === 0 && panes.length === 0) return { flying, moving }
  const ease = motionToken(root, "--desktop-ease") ?? "linear"
  const duration = durationToken(root, "--desktop-slow")
  const flight = durationToken(root, "--desktop-flight")
  slides.forEach(({ element, own }) => {
    flying.push(
      element.animate([{ transform: `translateX(${own}px)` }, { transform: "none" }], {
        duration,
        easing: ease,
        id: slideAnimation,
      }),
    )
  })
  panes.forEach(({ landed, before }) => {
    flying.push(...flyPane(landed, before, flight))
  })
  return { flying, moving }
}

export class FlipScope extends Component<{
  /** What the layout looks like, ignoring sizes; motion plays when it changes. */
  shape: string
  /**
   * A change of this is a cut, not a move: another layout taking the place
   * of the one shown — another window tab, say — lands at once, with no
   * flight from where the last one's panes stood.
   */
  cut?: string
  root: RefObject<HTMLElement | null>
  children: ReactNode
}> {
  private flights: Animation[] = []
  private moving: HTMLElement[] = []
  private scope: HTMLElement | null = null

  private release() {
    const flights = this.flights
    this.flights = []
    flights.forEach((flight) => flight.cancel())
    this.moving.forEach((pane) => pane.removeAttribute(marks.flying))
    this.moving = []
    if (this.scope) {
      reflectMark(this.scope, marks.flipping, false)
      this.scope.removeAttribute(marks.measuring)
    }
    this.scope = null
  }

  getSnapshotBeforeUpdate(
    previous: Readonly<{ shape: string; cut?: string }>,
  ): Snapshot | null {
    const root = this.props.root.current
    if (previous.shape === this.props.shape || !root) return null
    if (previous.cut !== this.props.cut) return null
    // Reduced motion makes the flight's duration zero: nothing to measure for.
    if (durationToken(root, "--desktop-flight") === 0) return null
    // Read mid-flight too: a change during a flight starts from where things are now.
    return { root, rects: measure(root) }
  }

  componentDidUpdate(
    previous: Readonly<{ shape: string }>,
    _state: unknown,
    snapshot: Snapshot | null,
  ) {
    const root = this.props.root.current
    if (root !== this.scope) {
      this.release()
      this.scope = root
    }
    if (!root) return
    const from = snapshot?.root === root ? snapshot.rects : null
    // A drag's preview put panes where this change puts them: measured through
    // it before the change, it is let go before measuring where they landed,
    // so a drop that lands where it previewed has nowhere to fly. With less
    // motion nothing is measured, but the preview goes all the same: left
    // on, it would draw the landed panes moved again.
    if (previous.shape !== this.props.shape) letGoOfDragPreview(root)
    if (!from && previous.shape === this.props.shape) return
    const previousFlights = this.flights
    const previousMoving = this.moving
    this.flights = []
    this.moving = []
    previousFlights.forEach((flight) => flight.cancel())
    previousMoving.forEach((pane) => pane.removeAttribute(marks.flying))
    if (!from) {
      reflectMark(root, marks.flipping, false)
      return
    }
    // Measure the new layout with content visible; the flight changes transforms only.
    reflectMark(root, marks.flipping, true)
    root.setAttribute(marks.measuring, "")
    let plan: ReturnType<typeof play>
    try {
      plan = play(root, from)
      plan.moving.forEach((pane) => pane.setAttribute(marks.flying, ""))
    } finally {
      root.removeAttribute(marks.measuring)
    }
    const { flying, moving } = plan
    this.flights = flying
    this.moving = moving
    if (flying.length === 0) {
      reflectMark(root, marks.flipping, false)
      return
    }
    Promise.all(flying.map((flight) => flight.finished))
      .then(() => {
        if (this.flights !== flying || this.scope !== root) return
        moving.forEach((pane) => pane.removeAttribute(marks.flying))
        this.moving = []
        reflectMark(root, marks.flipping, false)
      })
      .catch(() => undefined)
  }

  componentWillUnmount() {
    this.release()
  }

  render() {
    return this.props.children
  }
}
