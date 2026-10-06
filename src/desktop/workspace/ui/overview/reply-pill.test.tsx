// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest"
import { sizeReplyPill } from "./reply-pill"

function field(): HTMLTextAreaElement {
  const textarea = document.createElement("textarea")
  textarea.style.lineHeight = "18px"
  textarea.style.paddingTop = "5px"
  textarea.style.paddingBottom = "5px"
  return textarea
}

describe("sizeReplyPill", () => {
  it("leaves an empty draft at one line and does not read layout", () => {
    const textarea = field()
    textarea.style.height = "48px"
    const style = vi.spyOn(window, "getComputedStyle")
    Object.defineProperty(textarea, "scrollHeight", {
      get() {
        throw new Error("scrollHeight")
      },
    })
    sizeReplyPill(textarea, "")
    expect(style).not.toHaveBeenCalled()
    expect(textarea.style.height).toBe("")
  })

  it("grows a draft up to four lines", () => {
    const textarea = field()
    Object.defineProperty(textarea, "scrollHeight", { value: 40 })
    sizeReplyPill(textarea, "one line more")
    expect(textarea.style.height).toBe("40px")
  })

  it("stops at four lines and then scrolls", () => {
    const textarea = field()
    Object.defineProperty(textarea, "scrollHeight", { value: 400 })
    sizeReplyPill(textarea, "a draft long enough to pass four lines")
    // 18px line, 5px padding each side, four lines: 18 * 4 + 10.
    expect(textarea.style.height).toBe("82px")
  })
})
