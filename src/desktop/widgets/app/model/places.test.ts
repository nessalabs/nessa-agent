/** Display modes onto places (#349, Places and display modes; L17). */
import { describe, expect, it } from "vitest"
import { modeOf, offeredModes, requestMode } from "./places"

describe("display modes", () => {
  it("inline is inline; a pane and the window are fullscreen; pip is never offered", () => {
    expect(modeOf("inline")).toBe("inline")
    expect(modeOf("pane")).toBe("fullscreen")
    expect(modeOf("window")).toBe("fullscreen")
    expect(offeredModes).toEqual(["inline", "fullscreen"])
  })

  it("an inline view asking for fullscreen opens a pane, and stays inline", () => {
    expect(requestMode("inline", "fullscreen", undefined)).toEqual({
      open: "pane",
      answer: "inline",
    })
    expect(requestMode("inline", "fullscreen", ["inline", "fullscreen"])).toEqual({
      open: "pane",
      answer: "inline",
    })
  })

  it("a mode the app did not declare, pip, or anything from a pane or the window opens nothing", () => {
    expect(requestMode("inline", "fullscreen", ["inline"])).toEqual({ answer: "inline" })
    expect(requestMode("inline", "fullscreen", [])).toEqual({ answer: "inline" })
    expect(requestMode("inline", "pip", ["pip"])).toEqual({ answer: "inline" })
    expect(requestMode("inline", "inline", undefined)).toEqual({ answer: "inline" })
    for (const place of ["pane", "window"] as const)
      for (const mode of ["inline", "fullscreen", "pip"] as const)
        expect(requestMode(place, mode, undefined)).toEqual({ answer: "fullscreen" })
  })
})
