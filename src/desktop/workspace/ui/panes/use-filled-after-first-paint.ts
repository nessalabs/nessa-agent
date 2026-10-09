import { startTransition, useEffect, useState } from "react"

/**
 * A new pane answers on the frame it was asked for: its shell fades in at
 * once, and what it shows fills in over the next frames, under the fade, in
 * a transition React may slice, so a split never waits on what fills it.
 *
 * The fill is started from the next animation frame, inside a transition, so
 * it commits after that first paint and may wait longer while an urgent
 * update runs.
 */
export function useFilledAfterFirstPaint(): boolean {
  const [filled, setFilled] = useState(false)
  useEffect(() => {
    const handle = requestAnimationFrame(() => startTransition(() => setFilled(true)))
    return () => cancelAnimationFrame(handle)
  }, [])
  return filled
}
