import { afterEach, describe, expect, it, vi } from "vitest"
import { droppedText } from "./dropped-text"

afterEach(() => vi.unstubAllGlobals())

const data = (values: Record<string, string>) => ({
  getData: (type: string) => values[type] ?? "",
})
describe("droppedText", () => {
  it("retains selected text and whitespace", () => {
    expect(droppedText(data({ "text/plain": " hello\nworld " }))).toBe(" hello\nworld ")
  })
  it("uses actual link URLs instead of a browser's link title", () => {
    expect(
      droppedText(
        data({
          "text/uri-list": "# links\r\nhttps://example.com\r\nhttps://example.org",
          "text/plain": "Example",
        }),
      ),
    ).toBe("https://example.com\nhttps://example.org")
  })
  it("ignores comment-only URI data and unsupported content", () => {
    expect(droppedText(data({ "text/uri-list": "# no URL", "text/plain": "text" }))).toBe(
      "text",
    )
    expect(droppedText(data({}))).toBe("")
  })
})

it.each([true, false])(
  "keeps mixed image prose, while link-only HTML retains its URL (image=%s)",
  (hasImage) => {
    vi.stubGlobal(
      "DOMParser",
      class {
        parseFromString() {
          return {
            body: { textContent: " Selected prose " },
            querySelector: () => (hasImage ? {} : null),
          }
        }
      },
    )
    expect(
      droppedText(
        data({
          "text/html": hasImage
            ? '<p>Selected prose<img src="https://example.com/emoji.png"></p>'
            : '<a href="https://example.com">Selected prose</a>',
          "text/plain": " Selected prose ",
          "text/uri-list": "https://example.com",
        }),
      ),
    ).toBe(hasImage ? " Selected prose " : "https://example.com")
  },
)
