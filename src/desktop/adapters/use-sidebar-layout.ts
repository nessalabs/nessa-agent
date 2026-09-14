import { useCallback, useRef, useState } from "react"
import type { SplitViewLayout } from "@nessa-ui/react/split-view"

/** One presentation layout owns both sidebar sizes and visibility. */
export function useSidebarLayout() {
  const [layout, setLayout] = useState<SplitViewLayout>({
    left: 24,
    center: 52,
    right: 24,
  })
  const expanded = useRef({ left: 24, right: 24 })
  const changeLayout = useCallback((next: SplitViewLayout) => {
    for (const side of ["left", "right"] as const) {
      if (next[side] > 0) expanded.current[side] = next[side]
    }
    setLayout(next)
  }, [])
  const setOpen = useCallback((side: "left" | "right", open: boolean) => {
    setLayout((current) => {
      if (current[side] > 0 === open) return current
      const width = open ? Math.min(expanded.current[side], current.center - 20) : 0
      return { ...current, [side]: width, center: current.center + current[side] - width }
    })
  }, [])
  return {
    layout,
    changeLayout,
    setOpen,
    leftOpen: layout.left > 0,
    rightOpen: layout.right > 0,
  }
}
