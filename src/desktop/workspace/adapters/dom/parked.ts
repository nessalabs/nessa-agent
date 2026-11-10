import { useEffect, useLayoutEffect, useState, type RefObject } from "react"

/**
 * Whether a column that slid away has finished sliding and can be parked:
 * out of reach and skipped by style, layout and paint.
 *
 * Taking a big subtree out of reach changes what every descendant inherits
 * (`inert`, `visibility`), so the browser restyles all of them in that one
 * frame. Doing it on the key that folds the column made that frame miss the
 * budget, and doing it as the slide ends made the last frame miss it too
 * (#1177). Parked, the column is also `content-visibility: hidden`, set in
 * the same style change, so its descendants are skipped rather than
 * restyled. The column parks once its own transform transition has ended,
 * so the slide still paints its content all the way out. With no transition
 * (reduced motion, a column that starts away, a test DOM) it parks on the
 * next frame. Coming back unparks it in the same commit. Focus inside it
 * leaves on the fold's commit, as `inert` used to make it
 * (`useFocusLeavesOnFold`), so nothing types into a column on its way out.
 */
export function useParkedAfterSlide(
  ref: RefObject<HTMLElement | null>,
  away: boolean,
  /**
   * Marks on an ancestor that keep the column on screen though it is away:
   * it waits until none is there, then for its slide. A pane change holds
   * the sidebar in the row until the new boxes paint (`data-spare-held`),
   * and a peek handing off keeps it revealed (`data-peek`).
   */
  heldBy = "[data-spare-held], [data-peek]",
): boolean {
  const [parked, setParked] = useState(away)
  useEffect(() => {
    if (!away) {
      setParked(false)
      return
    }
    const node = ref.current
    let done = false
    const park = () => {
      if (!done) setParked(true)
    }
    // A frame on, the fold's style has applied and its transition has
    // started, unless something still holds it on screen (`heldBy`); its
    // slide starts after that.
    let frame = 0
    const look = () => {
      if (node?.closest(heldBy)) {
        frame = requestAnimationFrame(look)
        return
      }
      const slide = slideOf(node)
      if (!slide) return park()
      // A slide replaced while the column stays away (its transform changed
      // mid-slide) rejects: wait for the one that replaced it. A column that
      // came back has run this effect's cleanup, so it never parks.
      slide.finished.then(park, () => {
        if (!done) frame = requestAnimationFrame(look)
      })
    }
    frame = requestAnimationFrame(look)
    return () => {
      done = true
      cancelAnimationFrame(frame)
    }
  }, [away, ref, heldBy])
  return away && parked
}

function slideOf(node: HTMLElement | null): Animation | null {
  if (!node || typeof node.getAnimations !== "function") return null
  for (const animation of node.getAnimations()) {
    const property = (animation as Animation & { transitionProperty?: string })
      .transitionProperty
    if (property === "transform" && animation.playState !== "finished") return animation
  }
  return null
}

/**
 * Takes focus out of a column the moment it starts to slide away, before
 * paint, as `inert` on the fold's key did. Parking makes it inert only once
 * the slide has ended (`useParkedAfterSlide`).
 */
export function useFocusLeavesOnFold(ref: RefObject<HTMLElement | null>, away: boolean) {
  useLayoutEffect(() => {
    const node = ref.current
    const held = node?.ownerDocument.activeElement
    if (away && held instanceof HTMLElement && node?.contains(held)) held.blur()
  }, [away, ref])
}

/**
 * What `inert` did on the fold's key, for the slide before the column parks:
 * no press, click or context menu reaches its rows, and focus that lands in
 * it leaves. `pointer-events` would restyle every row, so these are capture
 * handlers on the column. Spread them on it instead of anything else that
 * reacts to the pointer there (an edge peek's holders), so the column's
 * reach has one owner while it slides.
 */
export function slidingGuards(sliding: boolean) {
  if (!sliding) return {}
  const stop = (event: { preventDefault(): void; stopPropagation(): void }) => {
    event.preventDefault()
    event.stopPropagation()
  }
  return {
    onPointerDownCapture: stop,
    onMouseDownCapture: stop,
    onClickCapture: stop,
    onContextMenuCapture: stop,
    onFocusCapture: (event: { target: EventTarget }) => {
      if (event.target instanceof HTMLElement) event.target.blur()
    },
  }
}
