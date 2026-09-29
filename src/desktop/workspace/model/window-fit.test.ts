import { describe, expect, it } from "vitest"
import { paneLimits } from "../../split-panes/model/pane-layout"
import {
  clampColumn,
  columnWidth,
  foldToFit,
  sessionListLimits,
  sidebarLimits,
  treeSidebarLimits,
  type SideColumns,
} from "./window-fit"

const open: SideColumns = {
  sidebarOpen: true,
  sessionListOpen: true,
  sidebarWidth: 240,
  sessionListWidth: 312,
}

describe("side column widths", () => {
  it("holds a dragged width within the column's limits", () => {
    expect(clampColumn(50, sidebarLimits)).toBe(sidebarLimits.min)
    expect(clampColumn(900, sidebarLimits)).toBe(sidebarLimits.max)
    expect(clampColumn(250, sidebarLimits)).toBe(250)
  })

  it("draws a column no wider than its share of the window, and never under its minimum", () => {
    expect(columnWidth(320, sidebarLimits, 1440)).toBe(288)
    expect(columnWidth(240, sidebarLimits, 1440)).toBe(240)
    expect(columnWidth(320, sidebarLimits, 600)).toBe(sidebarLimits.min)
    expect(columnWidth(420, sessionListLimits, 1000)).toBe(260)
    expect(columnWidth(256, treeSidebarLimits, 1000)).toBe(256)
  })
})

describe("folding the side columns to fit", () => {
  it("keeps both when the panes fit beside them", () => {
    expect(foldToFit(open, 1440, 2)).toBe(open)
  })

  it("folds the sidebar first", () => {
    const need = 2 * paneLimits.minWidth + paneLimits.gutter
    const width = need + 16 + 312 + 8 + 100
    expect(foldToFit(open, width, 2)).toEqual({ ...open, sidebarOpen: false })
  })

  it("folds the list too when the sidebar alone is not enough", () => {
    expect(foldToFit(open, 700, 2)).toEqual({
      ...open,
      sidebarOpen: false,
      sessionListOpen: false,
    })
  })

  it("returns the same value when nothing is left to fold", () => {
    const folded = { ...open, sidebarOpen: false, sessionListOpen: false }
    expect(foldToFit(folded, 400, 2)).toBe(folded)
  })

  it("leaves closed columns closed and only folds what is open", () => {
    const sidebarOnly = { ...open, sessionListOpen: false, sessionListWidth: 0 }
    expect(foldToFit(sidebarOnly, 500, 1)).toEqual({ ...sidebarOnly, sidebarOpen: false })
  })
})
