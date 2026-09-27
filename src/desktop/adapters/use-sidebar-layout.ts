import {
  fitSidebarWidths,
  LEFT_DEFAULT_WIDTH,
  LEFT_MAX_WIDTH,
  LEFT_MIN_WIDTH,
} from "./sidebar-sizing"
import { useCallback, useLayoutEffect, useRef, useState, type PointerEvent } from "react"
import {
  validatePanelGroupLayout,
  type SplitViewLayout,
} from "@nessa-ui/react/split-view"

/** One measured presentation layout owns sidebar widths and visibility. */
export function useSidebarLayout() {
  const groupRef = useRef<HTMLDivElement>(null)
  const availableWidth = useRef(0)
  const [layout, setLayout] = useState<SplitViewLayout>()
  const resizingSide = useRef<"left" | "right">("left")
  const expanded = useRef({ left: LEFT_DEFAULT_WIDTH, right: 400 })

  const constrain = useCallback((next: SplitViewLayout, width: number) => {
    const percent = (pixels: number) => (pixels / width) * 100
    const { left, center, right } = fitSidebarWidths(
      width,
      (next.left / 100) * width,
      (next.right / 100) * width,
      next.center === 0,
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
          minSize: percent(LEFT_MIN_WIDTH),
          maxSize: percent(LEFT_MAX_WIDTH),
          collapsible: true,
          collapsedSize: 0,
        },
        {
          panelId: "center",
          minSize: percent(350),
          maxSize: 100,
          collapsible: true,
          collapsedSize: 0,
        },
        {
          panelId: "right",
          minSize: percent(
            Math.min(160, Math.max(0, width - left - (center === 0 ? 0 : 350))),
          ),
          maxSize: 100,
          collapsible: true,
          collapsedSize: 0,
        },
      ],
    })
  }, [])

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
        const left = current ? (current.left / 100) * previousWidth : LEFT_DEFAULT_WIDTH
        const right = current ? (current.right / 100) * previousWidth : 400
        return constrain(
          {
            left: (left / width) * 100,
            center: current?.center === 0 ? 0 : ((width - left - right) / width) * 100,
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

  const snapWorkspace = useCallback(
    (event: PointerEvent<HTMLDivElement>) => {
      const group = groupRef.current
      if (!group || !event.currentTarget.hasPointerCapture(event.pointerId)) return
      const leftWidth = availableWidth.current * ((layout?.left ?? 0) / 100)
      const leftBorder = (group.children[1] as HTMLElement | undefined)?.offsetWidth ?? 0
      const workspaceWidth =
        event.clientX - group.getBoundingClientRect().left - leftWidth - leftBorder
      if (workspaceWidth < 350) {
        event.preventDefault()
        setLayout(
          (current) =>
            current && { left: current.left, center: 0, right: 100 - current.left },
        )
      }
    },
    [layout?.left],
  )

  const beginResize = useCallback((side: "left" | "right") => {
    resizingSide.current = side
  }, [])
  const changeLayout = useCallback(
    (next: SplitViewLayout) => {
      setLayout((current) => {
        // SplitView can propagate a drag across multiple neighbors. The right edge
        // is scoped to workspace/right: never accept a change to left from it.
        const fitted =
          current && resizingSide.current === "right"
            ? constrain({ ...next, left: current.left }, availableWidth.current)
            : next
        for (const side of ["left", "right"] as const) {
          if (fitted[side] > 0)
            expanded.current[side] = (fitted[side] / 100) * availableWidth.current
        }
        return fitted
      })
    },
    [constrain],
  )
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
        )
      })
    },
    [constrain],
  )
  return {
    groupRef,
    beginResize,
    snapWorkspace,
    workspaceCollapsed: layout?.center === 0,
    rightMinWidth: Math.min(
      160,
      Math.max(
        0,
        availableWidth.current * (1 - (layout?.left ?? 0) / 100) -
          (layout?.center === 0 ? 0 : 350),
      ),
    ),
    layout,
    changeLayout,
    setOpen,
    leftOpen: layout?.left !== 0,
    rightOpen: layout?.right !== 0,
  }
}
