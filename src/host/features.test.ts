import { describe, expect, it } from "vitest"

import { browser } from "./browser"
import { linux } from "./linux"
import { macos } from "./macos"
import { other } from "./other"
import { resolveHost } from "./resolve"

describe("resolveHost", () => {
  it("injects the browser host when there is no Tauri runtime", () => {
    expect(resolveHost("Mozilla/5.0 (Macintosh; Intel Mac OS X)", false)).toBe(
      browser,
    )
  })

  it("injects macOS features for a Mac Tauri user agent", () => {
    expect(resolveHost("Mozilla/5.0 (Macintosh; Intel Mac OS X)", true)).toBe(
      macos,
    )
    expect(macos.frost).toBe("native")
    expect(macos.capturePointerOnWestHandle).toBe(true)
  })

  it("injects Linux features for a Linux Tauri user agent", () => {
    expect(resolveHost("Mozilla/5.0 (X11; Linux x86_64)", true)).toBe(linux)
    expect(linux.frost).toBe("css")
    expect(linux.capturePointerOnWestHandle).toBe(false)
    expect(linux.westHandleClass).toContain("w-3")
  })

  it("injects the other host for an unmatched Tauri user agent", () => {
    expect(resolveHost("Mozilla/5.0 (Windows NT 10.0; Win64; x64)", true)).toBe(
      other,
    )
    expect(other.frost).toBe("css")
  })
})
