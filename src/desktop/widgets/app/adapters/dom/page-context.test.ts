// @vitest-environment jsdom
/**
 * The page's context an app is given (#349, host context): the design
 * system's tokens as the document resolves them, under MCP Apps' names; the
 * person's time zone; the platform it was told. What jsdom cannot show —
 * tokens resolved from the real style sheets and the theme's `dark` class —
 * is `mcp-apps.mjs --only inline`'s, which reads the context the app is told.
 */
import { afterEach, describe, expect, it } from "vitest"
import { readPageContext } from "./page-context"

afterEach(() => {
  document.documentElement.removeAttribute("style")
})

describe("the page's context", () => {
  it("names each token the page sets by MCP Apps' variable, and no other", () => {
    document.documentElement.style.setProperty("--foreground", "#171717")
    document.documentElement.style.setProperty("--radius-md", "8px")
    document.documentElement.style.setProperty("--unrelated", "1px")
    const page = readPageContext(document, "desktop")
    expect(page.styles).toEqual({
      "--color-text-primary": "#171717",
      "--color-background-inverse": "#171717",
      "--border-radius-md": "8px",
    })
    expect(page.platform).toBe("desktop")
    expect(page.timeZone).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone)
  })

  it("says nothing of a token the page does not set", () => {
    expect(readPageContext(document, "web").styles).toEqual({})
  })
})
