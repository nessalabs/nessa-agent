import { fitSidebarWidths } from "./sidebar-sizing"
import { useCallback, useLayoutEffect, useRef, useState } from "react"
import {
  validatePanelGroupLayout,
  type SplitViewLayout,
} from "@nessa-ui/react/split-view"

/** One measured presentation layout owns sidebar widths and visibility. */
export function useSidebarLayout() {
  const groupRef = useRef<HTMLDivElement>(null)
  const availableWidth = useRef(0)
  const [layout, setLayout] = useState<SplitViewLayout>()
  const expanded = useRef({ left: 200, right: 400 })

  const constrain = useCallback(
    (next: SplitViewLayout, width: number, priority: "left" | "right" = "left") => {
      const percent = (pixels: number) => (pixels / width) * 100
      const { left, right } = fitSidebarWidths(
        width,
        (next.left / 100) * width,
        (next.right / 100) * width,
        priority,
      )
      return validatePanelGroupLayout({
        layout: {
          left: percent(left),
          center: percent(width - left - right),
          right: percent(right),
        },
        panelConstraints: [
          {
            panelId: "left",
            minSize: percent(200),
            maxSize: percent(450),
            collapsible: true,
            collapsedSize: 0,
          },
          {
            panelId: "center",
            minSize: percent(350),
            maxSize: 100,
            collapsible: false,
            collapsedSize: 0,
          },
          {
            panelId: "right",
            minSize: percent(160),
            maxSize: 100,
            collapsible: true,
            collapsedSize: 0,
          },
        ],
      })
    },
    [],
  )

  useLayoutEffect(() => {
    const group = groupRef.current
    if (!group) return
    const measure = () => {
      const separators = Array.from(group.children).filter(
        (child) => child.getAttribute("data-slot") === "split-view-separator",
      )
      const width =
        group.clientWidth -
        separators.reduce((sum, child) => sum + (child as HTMLElement).offsetWidth, 0)
      if (width <= 0 || width === availableWidth.current) return
      const previousWidth = availableWidth.current
      availableWidth.current = width
      setLayout((current) => {
        const left = current ? (current.left / 100) * previousWidth : 200
        const right = current ? (current.right / 100) * previousWidth : 400
        return constrain(
          {
            left: (left / width) * 100,
            center: ((width - left - right) / width) * 100,
            right: (right / width) * 100,
          },
          width,
        )
      })
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(group)
    return () => observer.disconnect()
  }, [constrain])

  const changeLayout = useCallback((next: SplitViewLayout) => {
    for (const side of ["left", "right"] as const) {
      if (next[side] > 0)
        expanded.current[side] = (next[side] / 100) * availableWidth.current
    }
    setLayout(next)
  }, [])
  const setOpen = useCallback(
    (side: "left" | "right", open: boolean) => {
      setLayout((current) => {
        if (!current || current[side] > 0 === open) return current
        if (!open) {
          expanded.current[side] = (current[side] / 100) * availableWidth.current
          return { ...current, [side]: 0, center: current.center + current[side] }
        }
        for (const panel of ["left", "right"] as const) {
          if (current[panel] > 0)
            expanded.current[panel] = (current[panel] / 100) * availableWidth.current
        }
        const size = (expanded.current[side] / availableWidth.current) * 100
        return constrain(
          { ...current, [side]: size, center: current.center - size },
          availableWidth.current,
          side,
        )
      })
    },
    [constrain],
  )
  return {
    groupRef,
    layout,
    changeLayout,
    setOpen,
    leftOpen: layout?.left !== 0,
    rightOpen: layout?.right !== 0,
  }
}
