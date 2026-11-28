import { expect, it } from "vitest"
import {
  DEFAULT_ZOOM,
  parseZoom,
  zoomPercent,
  zoomSteps,
  zoomedIn,
  zoomedOut,
} from "./window-zoom"

it("reads only a known step, as a number or as it is stored", () => {
  expect(parseZoom("1.25")).toBe(1.25)
  expect(parseZoom(0.9)).toBe(0.9)
  for (const unknown of [null, undefined, "", " ", "big", "1.3", 3, Number.NaN, {}, "0"])
    expect(parseZoom(unknown)).toBe(DEFAULT_ZOOM)
})

it("steps through every size and stops at each end", () => {
  const up = [zoomSteps[0]]
  while (zoomedIn(up.at(-1)!) !== up.at(-1)) up.push(zoomedIn(up.at(-1)!))
  expect(up).toEqual([...zoomSteps])
  const down = [zoomSteps.at(-1)!]
  while (zoomedOut(down.at(-1)!) !== down.at(-1)) down.push(zoomedOut(down.at(-1)!))
  expect(down).toEqual([...zoomSteps].reverse())
})

it("holds the default among its steps, in order", () => {
  expect(zoomSteps).toContain(DEFAULT_ZOOM)
  expect([...zoomSteps].sort((a, b) => a - b)).toEqual([...zoomSteps])
})

it("says the zoom as a whole percentage", () => {
  expect(zoomSteps.map(zoomPercent)).toEqual([
    "80%",
    "90%",
    "100%",
    "110%",
    "125%",
    "150%",
  ])
})
