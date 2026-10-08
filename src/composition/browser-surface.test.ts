import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"

describe("the browser tab's handshake", () => {
  it("names the surface web", () => {
    const source = readFileSync(new URL("./browser.tsx", import.meta.url), "utf8")
    expect(source).toMatch(/connectBrowserSession\(\{[\s\S]*?surfaceKind: "web"/)
  })
})
