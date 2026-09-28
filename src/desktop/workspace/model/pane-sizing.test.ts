import { describe, expect, it } from "vitest"
import {
  paneLimits,
  paneShowing,
  singlePane,
  splitPane,
  type PaneLayout,
} from "./pane-layout"
import {
  arrange,
  columnsFit,
  edgeSides,
  fits,
  fitted,
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

describe("the one rule a change of layout is held to", () => {
  const roomy = { width: 1000, height: 800, spare: 0 }
  const { minWidth, minHeight, gutter } = paneLimits
  const one = singlePane("a")
  const beside = (layout: PaneLayout, side: "right" | "bottom") =>
    splitPane(layout, layout.focused, side, "x")

  it("takes a split that leaves every pane readable", () => {
    expect(arrange(one, beside(one, "right"), roomy)).toEqual({
      layout: beside(one, "right"),
      foldSidebar: false,
    })
  })

  it("holds columns to the readable width, the gutter counted, to the pixel", () => {
    const exact = { width: 2 * minWidth + gutter, height: 800, spare: 0 }
    expect(arrange(one, beside(one, "right"), exact)).not.toBeNull()
    expect(
      arrange(one, beside(one, "right"), { ...exact, width: exact.width - 1 }),
    ).toBeNull()
  })

  it("holds rows to the readable height, the gutter counted, to the pixel", () => {
    const exact = { width: 1000, height: 2 * minHeight + gutter, spare: 0 }
    expect(arrange(one, beside(one, "bottom"), exact)).not.toBeNull()
    expect(
      arrange(one, beside(one, "bottom"), { ...exact, height: exact.height - 1 }),
    ).toBeNull()
  })

  it("folds the sidebar when that is what makes a column fit, and says so", () => {
    const room = { width: 500, height: 800, spare: 248 }
    expect(arrange(one, beside(one, "right"), room)?.foldSidebar).toBe(true)
    // Folding gives width, never height.
    expect(arrange(one, beside(one, "bottom"), { ...room, height: 300 })).toBeNull()
  })

  it("rebalances a new column rather than leave one unreadable", () => {
    // Two columns, the second stacked: a third column beside the first halves it to 25%.
    const two = beside(one, "right")
    const stacked = splitPane(two, two.focused, "bottom", "y")
    const third = splitPane(stacked, 1, "left", "z")
    const room = { width: 1000, height: 800, spare: 0 }
    const placed = arrange(stacked, third, room)
    expect(placed).not.toBeNull()
    const shares = placed!.layout.columns.map((column) => column.share)
    const total = shares.reduce((sum, share) => sum + share, 0)
    const widths = shares.map((share) => ((room.width - 2 * gutter) * share) / total)
    expect(Math.min(...widths)).toBeGreaterThanOrEqual(minWidth - 0.5)
  })

  it("always takes a swap, which keeps every size", () => {
    const two = beside(one, "right")
    const swapped = { ...two, columns: [...two.columns].reverse() }
    expect(arrange(two, swapped, undefined)?.layout).toBe(swapped)
  })

  it("places nothing new with nothing measured: no room is not room", () => {
    expect(arrange(one, beside(one, "right"), undefined)).toBeNull()
  })

  it("refuses a change that changes nothing", () => {
    expect(arrange(one, one, roomy)).toBeNull()
  })
})

describe("fitting the panes to their room", () => {
  const { minWidth, minHeight, gutter } = paneLimits

  it("leaves a layout that fits as it is", () => {
    const two = splitPane(singlePane("a"), 1, "right", "b")
    expect(fitted(two, { width: 1000, height: 800 })).toBe(two)
    expect(fits(two, { width: 1000, height: 800 })).toBe(true)
  })

  it("raises a pane below the minimum to it, the room coming from the others", () => {
    const two = splitPane(singlePane("a"), 1, "right", "b")
    const lopsided = resizeEdge(two, { axis: "x", column: 0 }, 0.8, 10_000)
    const room = { width: 2 * minWidth + gutter + 100, height: 800 }
    const fit = fitted(lopsided, room)!
    const available = room.width - gutter
    const widths = fit.columns.map(
      (column) =>
        (available * column.share) /
        fit.columns.reduce((sum, each) => sum + each.share, 0),
    )
    expect(widths[1]).toBeCloseTo(minWidth)
    expect(widths[0]).toBeCloseTo(minWidth + 100)
  })

  it("cannot fit what even the minimum will not hold", () => {
    const stacked = splitPane(singlePane("a"), 1, "bottom", "b")
    expect(
      fitted(stacked, { width: 1000, height: 2 * minHeight + gutter - 1 }),
    ).toBeNull()
    expect(columnsFit(3, 3 * minWidth + 2 * gutter)).toBe(true)
    expect(columnsFit(3, 3 * minWidth + 2 * gutter - 1)).toBe(false)
  })

  it("measures an edge's two sides from the room, not from what is drawn", () => {
    const two = splitPane(singlePane("a"), 1, "right", "b")
    expect(
      edgeSides(two, { axis: "x", column: 0 }, { width: 1008, height: 800 }),
    ).toEqual({
      before: 500,
      after: 500,
    })
    expect(
      edgeSides(two, { axis: "x", column: 5 }, { width: 1008, height: 800 }),
    ).toBeNull()
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
