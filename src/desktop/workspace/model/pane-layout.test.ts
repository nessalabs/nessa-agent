import { describe, expect, it } from "vitest"
import {
  equalizePanes,
  focusPane,
  focusedPane,
  layoutShape,
  locate,
  movePane,
  nudgePane,
  paneCount,
  paneLimits,
  paneShowing,
  panesOf,
  removePane,
  showInPane,
  singlePane,
  splitPane,
  swapPanes,
  type PaneLayout,
} from "./pane-layout"

/** The layout as rows of session ids per column, the way a person would draw it. */
const drawn = (layout: PaneLayout) =>
  layout.columns.map((column) => column.panes.map((pane) => pane.sessionId))

const keyOf = (layout: PaneLayout, sessionId: string) => {
  const pane = paneShowing(layout, sessionId)
  if (!pane) throw new Error(`no pane shows ${sessionId}`)
  return pane.key
}

/** a | b over c: two columns, the second stacked. */
function threePanes(): PaneLayout {
  const one = singlePane("a")
  const two = splitPane(one, keyOf(one, "a"), "right", "b")
  return splitPane(two, keyOf(two, "b"), "bottom", "c")
}

/** a over d | b over c: the full grid. */
function fullGrid(): PaneLayout {
  const three = threePanes()
  return splitPane(three, keyOf(three, "a"), "bottom", "d")
}

describe("a pane layout", () => {
  it("starts as one focused pane", () => {
    const layout = singlePane("a")
    expect(drawn(layout)).toEqual([["a"]])
    expect(focusedPane(layout).sessionId).toBe("a")
    expect(paneCount(layout)).toBe(1)
  })

  it("splits right into a column that takes half the target's column, and focuses it", () => {
    const one = singlePane("a")
    const two = splitPane(one, keyOf(one, "a"), "right", "b")
    expect(drawn(two)).toEqual([["a"], ["b"]])
    expect(two.columns.map((column) => column.share)).toEqual([0.5, 0.5])
    expect(focusedPane(two).sessionId).toBe("b")
  })

  it("splits left before the target's column", () => {
    const one = singlePane("a")
    expect(drawn(splitPane(one, keyOf(one, "a"), "left", "b"))).toEqual([["b"], ["a"]])
  })

  it("splits down and up by halving the target pane only", () => {
    const three = threePanes()
    expect(drawn(three)).toEqual([["a"], ["b", "c"]])
    expect(three.columns[1].panes.map((pane) => pane.share)).toEqual([0.5, 0.5])
    const up = splitPane(three, keyOf(three, "c"), "top", "d")
    expect(drawn(up)).toEqual([["a"], ["b", "d", "c"]])
    expect(up.columns[1].panes.map((pane) => pane.share)).toEqual([0.5, 0.25, 0.25])
  })

  it("never shows one session twice", () => {
    const one = singlePane("a")
    expect(splitPane(one, keyOf(one, "a"), "right", "a")).toBe(one)
  })

  it("refuses a fifth pane: the grid is full at four", () => {
    const grid = fullGrid()
    expect(paneCount(grid)).toBe(paneLimits.maxPanes)
    expect(splitPane(grid, keyOf(grid, "a"), "bottom", "e")).toBe(grid)
    expect(splitPane(grid, keyOf(grid, "a"), "right", "e")).toBe(grid)
  })

  it("refuses a fourth column", () => {
    let layout = singlePane("a")
    layout = splitPane(layout, keyOf(layout, "a"), "right", "b")
    layout = splitPane(layout, keyOf(layout, "b"), "right", "c")
    expect(layout.columns).toHaveLength(paneLimits.maxColumns)
    expect(splitPane(layout, keyOf(layout, "c"), "right", "d")).toBe(layout)
    // Stacking is still allowed.
    expect(drawn(splitPane(layout, keyOf(layout, "c"), "bottom", "d"))).toEqual([
      ["a"],
      ["b"],
      ["c", "d"],
    ])
  })

  it("mints a key no pane has had, even after panes are closed", () => {
    const three = threePanes()
    const closed = removePane(three, keyOf(three, "c"))
    const reopened = splitPane(closed, keyOf(closed, "b"), "bottom", "c")
    const keys = panesOf(reopened).map((pane) => pane.key)
    expect(new Set(keys).size).toBe(keys.length)
    expect(keys).not.toContain(keyOf(three, "c"))
  })

  it("focuses only a pane it has, and does nothing when that one is focused", () => {
    const three = threePanes()
    expect(focusPane(three, 999)).toBe(three)
    const focused = focusPane(three, keyOf(three, "a"))
    expect(focusedPane(focused).sessionId).toBe("a")
    expect(focusPane(focused, keyOf(three, "a"))).toBe(focused)
  })

  it("shows a session in a pane in place, and changes nothing when it already does", () => {
    const one = singlePane("a")
    expect(drawn(showInPane(one, keyOf(one, "a"), "b"))).toEqual([["b"]])
    expect(showInPane(one, keyOf(one, "a"), "a")).toBe(one)
    expect(showInPane(one, 999, "b")).toBe(one)
  })
})

describe("closing a pane", () => {
  it("never closes the last pane", () => {
    const one = singlePane("a")
    expect(removePane(one, keyOf(one, "a"))).toBe(one)
  })

  it("gives a stacked pane's room to its neighbour in the column", () => {
    const three = threePanes()
    const closed = removePane(three, keyOf(three, "b"))
    expect(drawn(closed)).toEqual([["a"], ["c"]])
    expect(closed.columns[1].panes[0].share).toBe(1)
  })

  it("gives a lone column's room to the column beside it", () => {
    const three = threePanes()
    const closed = removePane(three, keyOf(three, "a"))
    expect(drawn(closed)).toEqual([["b", "c"]])
    expect(closed.columns[0].share).toBe(1)
  })

  it("moves focus to the next pane in reading order, or the previous", () => {
    const three = threePanes()
    const b = focusPane(three, keyOf(three, "b"))
    expect(focusedPane(removePane(b, keyOf(three, "b"))).sessionId).toBe("c")
    const c = focusPane(three, keyOf(three, "c"))
    expect(focusedPane(removePane(c, keyOf(three, "c"))).sessionId).toBe("b")
  })

  it("keeps focus where it was when another pane closes", () => {
    const three = focusPane(threePanes(), 1)
    expect(focusedPane(removePane(three, keyOf(three, "c"))).sessionId).toBe("a")
  })

  it("ignores a pane it does not have", () => {
    const three = threePanes()
    expect(removePane(three, 999)).toBe(three)
  })
})

describe("moving panes", () => {
  it("does nothing when a pane is dropped on itself", () => {
    const three = threePanes()
    for (const zone of ["center", "left", "right", "top", "bottom"] as const)
      expect(movePane(three, keyOf(three, "a"), keyOf(three, "a"), zone)).toBe(three)
  })

  it("swaps two panes in the middle, each place keeping its size", () => {
    const three = threePanes()
    const swapped = movePane(three, keyOf(three, "a"), keyOf(three, "c"), "center")
    expect(drawn(swapped)).toEqual([["c"], ["b", "a"]])
    expect(swapped.columns[0].share).toBe(three.columns[0].share)
    expect(focusedPane(swapped).sessionId).toBe("a")
  })

  it("moves a pane to a side of another, keeping its key", () => {
    const three = threePanes()
    const key = keyOf(three, "c")
    const moved = movePane(three, key, keyOf(three, "a"), "left")
    expect(drawn(moved)).toEqual([["c"], ["a"], ["b"]])
    expect(keyOf(moved, "c")).toBe(key)
    expect(paneCount(moved)).toBe(3)
  })

  it("moves a pane when the grid is full: a move is not a new pane", () => {
    const grid = fullGrid()
    const moved = movePane(grid, keyOf(grid, "d"), keyOf(grid, "c"), "bottom")
    expect(drawn(moved)).toEqual([["a"], ["b", "c", "d"]])
  })

  it("refuses a move that would make a fourth column", () => {
    let layout = singlePane("a")
    layout = splitPane(layout, keyOf(layout, "a"), "right", "b")
    layout = splitPane(layout, keyOf(layout, "b"), "right", "c")
    layout = splitPane(layout, keyOf(layout, "c"), "bottom", "d")
    expect(movePane(layout, keyOf(layout, "d"), keyOf(layout, "a"), "left")).toBe(layout)
  })

  it("ignores a pane or target it does not have", () => {
    const three = threePanes()
    expect(movePane(three, 999, keyOf(three, "a"), "left")).toBe(three)
    expect(movePane(three, keyOf(three, "a"), 999, "left")).toBe(three)
    expect(swapPanes(three, keyOf(three, "a"), 999)).toBe(three)
  })

  it("nudges up and down within a column", () => {
    const three = threePanes()
    const up = nudgePane(three, keyOf(three, "c"), "up")
    expect(drawn(up)).toEqual([["a"], ["c", "b"]])
    expect(nudgePane(three, keyOf(three, "b"), "up")).toBe(three)
  })

  it("nudges across to the nearest pane in the next column", () => {
    const three = threePanes()
    expect(drawn(nudgePane(three, keyOf(three, "c"), "left"))).toEqual([
      ["c"],
      ["b", "a"],
    ])
    expect(drawn(nudgePane(three, keyOf(three, "a"), "right"))).toEqual([
      ["b"],
      ["a", "c"],
    ])
  })

  it("steps a stacked pane out into its own column at the edge", () => {
    const three = threePanes()
    expect(drawn(nudgePane(three, keyOf(three, "c"), "right"))).toEqual([
      ["a"],
      ["b"],
      ["c"],
    ])
  })

  it("does nothing at the edge with nowhere to go", () => {
    const one = singlePane("a")
    expect(nudgePane(one, keyOf(one, "a"), "left")).toBe(one)
    const two = splitPane(one, keyOf(one, "a"), "right", "b")
    expect(nudgePane(two, keyOf(two, "b"), "right")).toBe(two)
  })
})

describe("evening out", () => {
  it("gives every column and every pane in a column the same share", () => {
    const three = threePanes()
    const uneven = splitPane(three, keyOf(three, "c"), "top", "d")
    const even = equalizePanes(uneven)
    expect(even.columns.map((column) => column.share)).toEqual([1, 1])
    expect(even.columns[1].panes.map((pane) => pane.share)).toEqual([1, 1, 1])
    expect(equalizePanes(even)).toBe(even)
  })

  it("keeps where every pane is", () => {
    const three = threePanes()
    expect(locate(equalizePanes(three), keyOf(three, "c"))).toEqual(
      locate(three, keyOf(three, "c")),
    )
  })
})

describe("a layout's shape", () => {
  it("changes when panes move, and not when they are resized or show another session", () => {
    const three = threePanes()
    const shape = layoutShape(three)
    expect(layoutShape(equalizePanes(three))).toBe(shape)
    expect(layoutShape(showInPane(three, keyOf(three, "a"), "z"))).toBe(shape)
    expect(layoutShape(swapPanes(three, keyOf(three, "a"), keyOf(three, "c")))).not.toBe(
      shape,
    )
    expect(layoutShape(removePane(three, keyOf(three, "c")))).not.toBe(shape)
  })
})
