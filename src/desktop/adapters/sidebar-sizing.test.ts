import { describe, expect, it } from "vitest"
import { fitSidebarWidths } from "./sidebar-sizing"

describe("desktop pixel allocation", () => {
  it("starts left at 250px, right at 400px, and gives the remainder to the workspace", () => {
    expect(fitSidebarWidths(1100 - 2)).toEqual({ left: 250, center: 448, right: 400 })
  })
  it("bounds the left and lets the right grow only into workspace surplus", () => {
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
  it("gives the right its 200px minimum before the left, narrowing the left first", () => {
    expect(fitSidebarWidths(798, 400, 400)).toEqual({
      left: 248,
      center: 350,
      right: 200,
    })
  })
  it("closes the left when it cannot keep 200px beside the right's minimum", () => {
    expect(fitSidebarWidths(663)).toEqual({ left: 0, center: 350, right: 313 })
    expect(fitSidebarWidths(749, 250, 400)).toEqual({ left: 0, center: 350, right: 399 })
    expect(fitSidebarWidths(750, 250, 400)).toEqual({
      left: 200,
      center: 350,
      right: 200,
    })
  })
  it("closes the right rather than shrinking it below 200px", () => {
    expect(fitSidebarWidths(549, 0, 400)).toEqual({ left: 0, center: 549, right: 0 })
    expect(fitSidebarWidths(550, 0, 400)).toEqual({ left: 0, center: 350, right: 200 })
  })
  it("narrows the left to fit before closing it when the right is shut", () => {
    expect(fitSidebarWidths(560, 250, 0)).toEqual({ left: 210, center: 350, right: 0 })
    expect(fitSidebarWidths(549, 250, 0)).toEqual({ left: 0, center: 549, right: 0 })
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
  })
  it("returns freed right-panel width to the workspace", () => {
    expect(fitSidebarWidths(1098, 300, 0)).toEqual({ left: 300, center: 798, right: 0 })
  })
  it("gives the entire workspace to right when the workspace snaps closed", () => {
    expect(fitSidebarWidths(1098, 200, 600, true)).toEqual({
      left: 200,
      center: 0,
      right: 898,
    })
    expect(fitSidebarWidths(1098, 200, 548)).toEqual({
      left: 200,
      center: 350,
      right: 548,
    })
  })
})
