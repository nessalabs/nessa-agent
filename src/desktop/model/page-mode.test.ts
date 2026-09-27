import { describe, expect, it } from "vitest"
import { nextPageMode, pageCloseAt, pageOpenAt } from "./page-mode"

describe("nextPageMode", () => {
  it.each([
    ["card", false, pageOpenAt, true],
    ["card", false, pageOpenAt - 1, false],
    ["page", true, pageCloseAt, false],
    ["page", true, pageCloseAt + 1, true],
  ])("from the %s at %i lines", (_showing, showingPage, lines, next) => {
    expect(nextPageMode(showingPage, lines)).toBe(next)
  })

  it("holds either layout between the thresholds", () => {
    for (let lines = pageCloseAt + 1; lines < pageOpenAt; lines += 1) {
      expect(nextPageMode(false, lines)).toBe(false)
      expect(nextPageMode(true, lines)).toBe(true)
    }
  })
})
