import { useEffect, useState, type RefObject } from "react"

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
 * next frame. Coming back unparks it in the same commit.
 */
export function useParkedAfterSlide(
  ref: RefObject<HTMLElement | null>,
  away: boolean,
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
    // started. A pane change that folds it keeps it in the row until the new
    // boxes paint (`data-spare-held`); its slide starts after that.
    let frame = 0
    const look = () => {
      if (node?.closest(`[${spareHeld}]`)) {
        frame = requestAnimationFrame(look)
        return
      }
      const slide = slideOf(node)
      if (!slide) return park()
      // A cancelled slide (the column came back) rejects; `away` is then false.
      slide.finished.then(park, () => {})
    }
    frame = requestAnimationFrame(look)
    return () => {
      done = true
      cancelAnimationFrame(frame)
    }
  }, [away, ref])
  return away && parked
}

/** The workspace's mark while a pane change holds the sidebar in the row (`split-panes.tsx`). */
const spareHeld = "data-spare-held"

function slideOf(node: HTMLElement | null): Animation | null {
  if (!node || typeof node.getAnimations !== "function") return null
  for (const animation of node.getAnimations()) {
    const property = (animation as Animation & { transitionProperty?: string })
      .transitionProperty
    if (property === "transform" && animation.playState !== "finished") return animation
  }
  return null
}
