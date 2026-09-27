import { describe, expect, it } from "vitest"
import {
  paneLimits,
  paneShowing,
  singlePane,
  splitPane,
  type PaneLayout,
} from "./pane-layout"
import {
  canPlace,
  needsSidebarRoom,
  placeBeside,
  placements,
  resizeEdge,
} from "./pane-sizing"

const keyOf = (layout: PaneLayout, sessionId: string) =>
  paneShowing(layout, sessionId)!.key

function threePanes(): PaneLayout {
  const one = singlePane("a")
  const two = splitPane(one, keyOf(one, "a"), "right", "b")
  return splitPane(two, keyOf(two, "b"), "bottom", "c")
}

describe("placements", () => {
  it("draws one pane over the whole workspace, in the corner", () => {
    const { panes, edges } = placements(singlePane("a").columns)
    expect(panes).toEqual([
      {
        key: 1,
        x: 0,
        width: 1,
        column: 0,
        columns: 1,
        y: 0,
        height: 1,
        row: 0,
        rows: 1,
        corner: true,
      },
    ])
    expect(edges).toEqual([])
  })

  it("places columns and stacked panes by their shares, with an edge between each pair", () => {
    const { panes, edges } = placements(threePanes().columns)
    expect(panes.map((pane) => [pane.x, pane.width, pane.y, pane.height])).toEqual([
      [0, 0.5, 0, 1],
      [0.5, 0.5, 0, 0.5],
      [0.5, 0.5, 0.5, 0.5],
    ])
    expect(panes.map((pane) => pane.corner)).toEqual([true, false, false])
    expect(edges.map((edge) => edge.edge)).toEqual([
      { axis: "x", column: 0 },
      { axis: "y", column: 1, row: 0 },
    ])
  })

  it("names each edge by what it sits between, so it keeps its identity", () => {
    const edges = placements(threePanes().columns).edges
    const ids = edges.map((edge) => edge.id)
    expect(new Set(ids).size).toBe(ids.length)
    const layout = threePanes()
    expect(edges.map((edge) => [edge.before, edge.after])).toEqual([
      [keyOf(layout, "a"), keyOf(layout, "b")],
      [keyOf(layout, "b"), keyOf(layout, "c")],
    ])
  })
})

describe("whether a pane fits", () => {
  const roomy = { width: 1000, height: 800, spare: 0 }

  it("fits anywhere within the limits when nothing was measured", () => {
    const one = singlePane("a")
    expect(canPlace(one, "right", 1)).toBe(true)
    expect(canPlace(one, "bottom", 1)).toBe(true)
  })

  it("does not fit beside a pane that is not there", () => {
    expect(canPlace(singlePane("a"), "right", 999, roomy)).toBe(false)
  })

  it("needs half the target's width to be readable for a column", () => {
    const one = singlePane("a")
    const narrow = { width: 2 * paneLimits.minWidth - 1, height: 800, spare: 0 }
    expect(canPlace(one, "right", 1, narrow)).toBe(false)
    expect(canPlace(one, "right", 1, { ...narrow, width: 2 * paneLimits.minWidth })).toBe(
      true,
    )
  })

  it("counts the width the sidebar would give up", () => {
    const one = singlePane("a")
    const room = { width: 500, height: 800, spare: 248 }
    expect(canPlace(one, "right", 1, room)).toBe(true)
    expect(needsSidebarRoom("right", room)).toBe(true)
    expect(needsSidebarRoom("bottom", room)).toBe(false)
    expect(needsSidebarRoom("right", roomy)).toBe(false)
    expect(needsSidebarRoom("right", undefined)).toBe(false)
  })

  it("needs half the target's height to be readable for a row", () => {
    const one = singlePane("a")
    expect(
      canPlace(one, "bottom", 1, { ...roomy, height: 2 * paneLimits.minHeight - 1 }),
    ).toBe(false)
    expect(
      canPlace(one, "bottom", 1, { ...roomy, height: 2 * paneLimits.minHeight }),
    ).toBe(true)
  })

  it("lets a pane move within a full grid", () => {
    const three = threePanes()
    const grid = splitPane(three, keyOf(three, "a"), "bottom", "d")
    expect(canPlace(grid, "bottom", keyOf(grid, "c"), roomy)).toBe(false)
    expect(canPlace(grid, "bottom", keyOf(grid, "c"), roomy, keyOf(grid, "d"))).toBe(true)
  })

  it("falls back to the other axis, and to nowhere", () => {
    const one = singlePane("a")
    const narrow = { width: 400, height: 800, spare: 0 }
    expect(placeBeside(one, 1, "right", narrow)).toBe("bottom")
    expect(placeBeside(one, 1, "bottom", { width: 800, height: 300, spare: 0 })).toBe(
      "right",
    )
    expect(placeBeside(one, 1, "right", { width: 400, height: 300, spare: 0 })).toBeNull()
    expect(placeBeside(one, 1, "right", roomy)).toBe("right")
  })
})

describe("resizing an edge", () => {
  it("gives the leading side the asked fraction of the pair", () => {
    const one = singlePane("a")
    const two = splitPane(one, 1, "right", "b")
    const resized = resizeEdge(two, { axis: "x", column: 0 }, 0.7, 1000)
    const [a, b] = resized.columns.map((column) => column.share)
    expect(a / (a + b)).toBeCloseTo(0.7)
    expect(a + b).toBeCloseTo(1)
  })

  it("holds both sides at the readable minimum", () => {
    const two = splitPane(singlePane("a"), 1, "right", "b")
    const pair = 1000
    const squeezed = resizeEdge(two, { axis: "x", column: 0 }, 0.05, pair)
    const [a, b] = squeezed.columns.map((column) => column.share)
    expect((a / (a + b)) * pair).toBeCloseTo(paneLimits.minWidth)
    const other = resizeEdge(two, { axis: "x", column: 0 }, 0.99, pair)
    const [c, d] = other.columns.map((column) => column.share)
    expect((d / (c + d)) * pair).toBeCloseTo(paneLimits.minWidth)
  })

  it("splits the pair evenly when it is too small for both minimums", () => {
    const two = splitPane(singlePane("a"), 1, "right", "b")
    const tight = resizeEdge(two, { axis: "x", column: 0 }, 0.9, 400)
    expect(tight.columns[0].share).toBeCloseTo(tight.columns[1].share)
  })

  it("resizes two stacked panes by height, leaving the rest alone", () => {
    const three = threePanes()
    const resized = resizeEdge(three, { axis: "y", column: 1, row: 0 }, 0.25, 1000)
    expect(resized.columns[0]).toBe(three.columns[0])
    const [b, c] = resized.columns[1].panes.map((pane) => pane.share)
    expect(b / (b + c)).toBeCloseTo(0.25)
  })

  it("ignores an edge that is not there", () => {
    const three = threePanes()
    expect(resizeEdge(three, { axis: "x", column: 1 }, 0.5, 1000)).toBe(three)
    expect(resizeEdge(three, { axis: "y", column: 0, row: 0 }, 0.5, 1000)).toBe(three)
  })
})
