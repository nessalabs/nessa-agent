import { describe, expect, it } from "vitest"
import { fitSidebarWidths } from "./sidebar-sizing"

describe("desktop pixel allocation", () => {
  it("starts right at 400px and assigns the remainder to the workspace", () => {
    expect(fitSidebarWidths(1100 - 2)).toEqual({ left: 200, center: 498, right: 400 })
  })
  it("bounds the left and lets the right consume only workspace surplus", () => {
    expect(fitSidebarWidths(1098, 900, 900)).toEqual({
      left: 450,
      center: 350,
      right: 298,
    })
    expect(fitSidebarWidths(1098, 100, 900)).toEqual({
      left: 200,
      center: 350,
      right: 548,
    })
  })
  it("fits right to the available workspace surplus without collapsing left", () => {
    expect(fitSidebarWidths(663)).toEqual({ left: 200, center: 350, right: 113 })
  })
  it("preserves explicitly collapsed sidebars", () => {
    expect(fitSidebarWidths(1098, 0, 0)).toEqual({ left: 0, center: 1098, right: 0 })
  })
  it("stops right growth at the workspace minimum without moving left", () => {
    for (const right of [400, 600, 1000]) {
      expect(fitSidebarWidths(1098, 450, right)).toEqual({
        left: 450,
        center: 350,
        right: 298,
      })
    }
    expect(fitSidebarWidths(798, 400, 400)).toEqual({ left: 400, center: 350, right: 48 })
  })
  it("returns freed right-panel width to the workspace", () => {
    expect(fitSidebarWidths(1098, 300, 0)).toEqual({ left: 300, center: 798, right: 0 })
  })
})
