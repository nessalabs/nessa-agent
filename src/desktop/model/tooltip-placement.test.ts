import { describe, expect, it } from "vitest"
import { placeTooltip } from "./tooltip-placement"

const window = { width: 1000, height: 700 }
const tip = { width: 120, height: 22 }
const button = (left: number, top: number) => ({ left, top, width: 28, height: 28 })

describe("where a tooltip sits", () => {
  it("centres under a control, a gap away, never over it", () => {
    const placed = placeTooltip(button(440, 10), tip, window)
    expect(placed).toEqual({ x: 394, y: 44, side: "below" })
    expect(placed.y).toBeGreaterThanOrEqual(10 + 28)
  })

  it("sits above a control that prefers it, like a composer chip", () => {
    expect(placeTooltip(button(440, 600), tip, window, { prefer: "above" })).toEqual({
      x: 394,
      y: 572,
      side: "above",
    })
  })

  it("flips to the other side when the preferred one runs out of window", () => {
    expect(placeTooltip(button(440, 660), tip, window).side).toBe("above")
    expect(placeTooltip(button(440, 12), tip, window, { prefer: "above" }).side).toBe(
      "below",
    )
  })

  it("shifts along to stay inside the window at either edge", () => {
    expect(placeTooltip(button(0, 10), tip, window).x).toBe(8)
    expect(placeTooltip(button(980, 10), tip, window).x).toBe(872)
  })

  it("flips away from the window's traffic lights", () => {
    const lights = { left: 0, top: 0, width: 84, height: 36 }
    const placed = placeTooltip(button(20, 60), tip, window, {
      prefer: "above",
      avoid: lights,
    })
    expect(placed.side).toBe("below")
  })
})

describe("where a popover sits by its chip", () => {
  const popover = { width: 248, height: 120 }
  const chip = (left: number, top: number) => ({ left, top, width: 40, height: 30 })
  const beside = {
    prefer: "above",
    align: "end",
    spacing: { gap: 10, margin: 12 },
  } as const

  it("sits above with its trailing edge on the chip's, its own gap away", () => {
    expect(placeTooltip(chip(700, 600), popover, window, beside)).toEqual({
      x: 740 - 248,
      y: 600 - 10 - 120,
      side: "above",
    })
  })

  it("shifts along to stay inside the window by its own margin", () => {
    expect(placeTooltip(chip(20, 600), popover, window, beside).x).toBe(12)
    expect(placeTooltip(chip(990, 600), popover, window, beside).x).toBe(1000 - 12 - 248)
  })

  it("goes below only when above would come nearer the window's edge than the margin", () => {
    expect(placeTooltip(chip(700, 11 + 10 + 120), popover, window, beside).side).toBe(
      "below",
    )
    expect(placeTooltip(chip(700, 12 + 10 + 120), popover, window, beside).side).toBe(
      "above",
    )
    // Room for neither: it stays on the side it prefers.
    expect(
      placeTooltip(chip(700, 60), popover, { width: 1000, height: 200 }, beside).side,
    ).toBe("above")
  })
})
