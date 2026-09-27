import { describe, expect, it } from "vitest"

import { linkNotice } from "./link-notice"
import type { LinkNotOpened } from "../../host"

function refused(url: string): LinkNotOpened {
  return { url, reason: "refused", detail: null }
}

describe("linkNotice", () => {
  it("says which link was not opened, so one of several in a message is identifiable", () => {
    expect(linkNotice(refused("file:///etc/passwd")).description).toContain(
      "file:///etc/passwd",
    )
  })

  it("keeps the scheme and host when a long link is shortened", () => {
    const long = `vscode://file/Users/someone/${"deep/".repeat(40)}file.rs`
    const { description } = linkNotice(refused(long))
    expect(description).toContain("vscode://file/Users/someone")
    expect(description).toContain("…")
    expect(description.length).toBeLessThan(200)
  })

  it("distinguishes a refusal from a browser that would not start", () => {
    const failed = linkNotice({
      url: "https://anthropic.com/",
      reason: "opener-failed",
      detail: "no xdg-open on PATH",
    })
    expect(failed.title).not.toEqual(linkNotice(refused("file:///x")).title)
    // The host's own error is for the diagnostics, not the person.
    expect(failed.description).not.toContain("xdg-open")
  })
})
