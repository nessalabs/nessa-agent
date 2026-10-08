// @vitest-environment jsdom
import { renderToStaticMarkup } from "react-dom/server"
import { expect, it } from "vitest"
import { StatusGlyph } from "./status-glyph"

const glyph = (node: React.ReactElement) => {
  const host = document.createElement("div")
  host.innerHTML = renderToStaticMarkup(node)
  return host.firstElementChild as HTMLElement | null
}

it("says what it shows, unread included, to a reader and in its tooltip", () => {
  const unread = glyph(<StatusGlyph status="unread" />)
  expect(unread?.getAttribute("role")).toBe("img")
  expect(unread?.getAttribute("aria-label")).toBe("Unread")
  expect(unread?.dataset.tooltip).toBe("Unread")
  expect(glyph(<StatusGlyph status="needs-you" />)?.getAttribute("aria-label")).toBe(
    "Needs you",
  )
})

it("says nothing beside words that already say it, and draws a point at its own width when asked", () => {
  const point = glyph(<StatusGlyph status="needs-you" flush decorative />)
  expect(point?.getAttribute("aria-hidden")).toBe("true")
  expect(point?.hasAttribute("role")).toBe(false)
  expect(point?.hasAttribute("aria-label")).toBe(false)
  expect(point?.hasAttribute("data-tooltip")).toBe(false)
  expect(point?.hasAttribute("data-flush")).toBe(true)
  expect(glyph(<StatusGlyph status="running" />)?.hasAttribute("data-flush")).toBe(false)
})
