/** The host context an app is given, and what of it changed (#349, L6, L13). */
import { describe, expect, it } from "vitest"
import { appHostContext, changedContext, inlineMaxHeight } from "./host-context"

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
