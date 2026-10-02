/** Display modes onto places (#349, Places and display modes; L17). */
import { describe, expect, it } from "vitest"
import { appHostContext, changedContext, inlineMaxHeight } from "./host-context"
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

describe("host context", () => {
  const context = {
    theme: "dark" as const,
    locale: "en-GB",
    size: { width: 480, height: 320 },
    safeArea: { top: 0, right: 0, bottom: 0, left: 0 },
  }
  const page = {
    styles: { "--color-text-primary": "#fafafa" },
    timeZone: "Europe/London",
    platform: "desktop" as const,
  }

  it("translates the widget host's context, field for field, with the app's own", () => {
    expect(appHostContext("pane", context, page, { name: "show" })).toEqual({
      toolInfo: { tool: { name: "show" } },
      theme: "dark",
      styles: { variables: { "--color-text-primary": "#fafafa" } },
      displayMode: "fullscreen",
      availableDisplayModes: ["inline", "fullscreen"],
      containerDimensions: { width: 480, height: 320 },
      locale: "en-GB",
      timeZone: "Europe/London",
      platform: "desktop",
      safeAreaInsets: { top: 0, right: 0, bottom: 0, left: 0 },
    })
  })

  it("inline, the width is fixed and the height the app's, up to a limit", () => {
    expect(
      appHostContext("inline", context, page, undefined).containerDimensions,
    ).toEqual({
      width: 480,
      maxHeight: inlineMaxHeight,
    })
    expect(
      appHostContext("inline", { ...context, size: null }, page, undefined)
        .containerDimensions,
    ).toEqual({ maxHeight: inlineMaxHeight })
    expect(
      appHostContext("window", { ...context, size: null }, page, undefined)
        .containerDimensions,
    ).toEqual({})
  })

  it("L13: a change says only the fields that changed, and nothing when none did", () => {
    const before = appHostContext("pane", context, page, undefined)
    expect(changedContext(before, appHostContext("pane", context, page, undefined))).toBe(
      undefined,
    )
    expect(
      changedContext(
        before,
        appHostContext(
          "pane",
          { ...context, theme: "light", size: { width: 1, height: 2 } },
          page,
          undefined,
        ),
      ),
    ).toEqual({ theme: "light", containerDimensions: { width: 1, height: 2 } })
  })
})
