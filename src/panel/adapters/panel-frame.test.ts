// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest"

import { publishWindowSize } from "./panel-frame"

describe("publishWindowSize", () => {
  afterEach(() => document.documentElement.removeAttribute("style"))

  const written = () => {
    const style = document.documentElement.style
    return {
      width: style.getPropertyValue("--nessa-window-width"),
      height: style.getPropertyValue("--nessa-window-height"),
    }
  }

  it("writes the window's size for the panel and the load fallback", () => {
    publishWindowSize({ width: 420, height: 835 })
    expect(written()).toEqual({ width: "420px", height: "835px" })
  })

  it.each([
    ["no size", null],
    ["a zero width", { width: 0, height: 835 }],
    ["a zero height", { width: 420, height: 0 }],
    ["an infinite width", { width: Infinity, height: 835 }],
    ["an infinite height", { width: 420, height: Infinity }],
  ])("writes nothing for %s", (_, size) => {
    publishWindowSize(size)
    expect(written()).toEqual({ width: "", height: "" })
  })
})
