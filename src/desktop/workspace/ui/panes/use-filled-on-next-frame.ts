import { startTransition, useEffect, useState } from "react"

/**
 * A new pane's shell is on the frame it was asked for. What it shows fills
 * in on the next one, in a transition React may slice. A conversation pane
 * and a widget pane share this (`pane.tsx`, `widget-pane.tsx`). The widget
 * body would otherwise be measured before the flight can hide the panes
 * (`widgets.test.tsx`).
 */
export function useFilledOnNextFrame(): boolean {
  const [filled, setFilled] = useState(false)
  useEffect(() => {
    const handle = requestAnimationFrame(() => startTransition(() => setFilled(true)))
    return () => cancelAnimationFrame(handle)
  }, [])
  return filled
}
