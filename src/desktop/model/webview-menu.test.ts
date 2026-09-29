import { describe, expect, it } from "vitest"
import { webviewMenuOpens, type RightClick } from "./webview-menu"

const elsewhere: RightClick = {
  inEditableText: false,
  onSelectedText: false,
  withOption: false,
}

describe("when the webview's own menu opens", () => {
  it("does not open on the window's chrome", () => {
    expect(webviewMenuOpens(elsewhere, false)).toBe(false)
    expect(webviewMenuOpens(elsewhere, true)).toBe(false)
  })

  it("opens in editable text and on selected text, in every build", () => {
    for (const inspectable of [false, true]) {
      expect(webviewMenuOpens({ ...elsewhere, inEditableText: true }, inspectable)).toBe(
        true,
      )
      expect(webviewMenuOpens({ ...elsewhere, onSelectedText: true }, inspectable)).toBe(
        true,
      )
    }
  })

  it("opens anywhere with ⌥ held only in a development build", () => {
    expect(webviewMenuOpens({ ...elsewhere, withOption: true }, true)).toBe(true)
    expect(webviewMenuOpens({ ...elsewhere, withOption: true }, false)).toBe(false)
  })
})
