import { expect, it } from "vitest"
import {
  closedPanel,
  PANEL_DEFAULT_WIDTH,
  PANEL_MAX_WIDTH,
  PANEL_MIN_WIDTH,
  panelWidth,
  parsePanel,
  serializePanel,
} from "./right-panel"

it("reads back the place it remembered, and anything else as a closed panel", () => {
  expect(parsePanel(serializePanel({ open: true, width: 420 }))).toEqual({
    open: true,
    width: 420,
  })
  expect(parsePanel(null)).toEqual(closedPanel)
  expect(parsePanel("not json")).toEqual(closedPanel)
  expect(parsePanel('{"open":"yes"}')).toEqual({
    open: false,
    width: PANEL_DEFAULT_WIDTH,
  })
  expect(parsePanel('{"open":true,"width":9000}')).toEqual({
    open: true,
    width: PANEL_MAX_WIDTH,
  })
})

it("is drawn within its limits and never more than its share of the window", () => {
  expect(panelWidth(100, 2000)).toBe(PANEL_MIN_WIDTH)
  expect(panelWidth(500, 2000)).toBe(500)
  expect(panelWidth(500, 1000)).toBe(340)
  // A narrow window still leaves it its minimum.
  expect(panelWidth(500, 600)).toBe(PANEL_MIN_WIDTH)
})
