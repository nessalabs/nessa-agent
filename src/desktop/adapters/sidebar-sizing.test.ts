import { describe, expect, it } from "vitest"
import { fitSidebarWidths } from "./sidebar-sizing"

describe("desktop pixel allocation", () => {
  it("assigns the default remainder to the right after both borders", () => {
    expect(fitSidebarWidths(1100 - 2)).toEqual({ left: 200, center: 350, right: 548 })
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
  it("releases the right when its expanded minimum cannot fit", () => {
    expect(fitSidebarWidths(663)).toEqual({ left: 200, center: 463, right: 0 })
  })
  it("preserves explicitly collapsed sidebars and releases left on narrow windows", () => {
    expect(fitSidebarWidths(1098, 0, 0)).toEqual({ left: 0, center: 1098, right: 0 })
    expect(fitSidebarWidths(500)).toEqual({ left: 0, center: 500, right: 0 })
  })
  it("opens the requested right panel in a narrow browser by releasing left", () => {
    expect(fitSidebarWidths(663, 200, 240, "right")).toEqual({
      left: 0,
      center: 423,
      right: 240,
    })
    expect(fitSidebarWidths(663, 200, 240, "left")).toEqual({
      left: 200,
      center: 463,
      right: 0,
    })
  })
  it("shrinks left instead of closing it when both expanded minima can fit", () => {
    expect(fitSidebarWidths(798, 450, 240, "right")).toEqual({
      left: 288,
      center: 350,
      right: 160,
    })
  })
})
