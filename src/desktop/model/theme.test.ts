import { describe, expect, it } from "vitest"
import { defaultDesktopTheme, desktopThemes, parseDesktopTheme } from "./theme"

describe("parseDesktopTheme", () => {
  it("keeps every known theme", () => {
    for (const theme of desktopThemes) expect(parseDesktopTheme(theme.id)).toBe(theme.id)
  })

  it.each([null, undefined, "", "purple", "toString", 3])(
    "falls back to the default for %s",
    (value) => {
      expect(parseDesktopTheme(value)).toBe(defaultDesktopTheme)
    },
  )

  it("defaults to a neutral theme", () => {
    expect(defaultDesktopTheme).toBe("graphite")
  })
})
