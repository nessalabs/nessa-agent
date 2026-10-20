// @vitest-environment jsdom
import { readFileSync } from "node:fs"
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { RAIL_LOADING_DELAY_MS, RailView, railItems, railWidth } from "./side-rail"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

beforeEach(() => {
  vi.useFakeTimers()
  // The loading text asks for reduced motion; jsdom has no matchMedia.
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia
  // Nor the Web Animations its sweep runs on.
  Element.prototype.animate ??= function () {
    return { cancel() {}, finished: Promise.resolve() } as unknown as Animation
  }
})

afterEach(() => {
  vi.useRealTimers()
})

const files = railItems.find((item) => item.id === "files") ?? railItems[0]

it("opens an item's view empty, and shows the loader only once a load runs past three seconds", async () => {
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  await act(async () => root.render(<RailView item={files} />))

  const view = host.querySelector<HTMLElement>(".rail-view")
  expect(view?.getAttribute("aria-label")).toBe("Files")
  // No placeholder and no loader to flash before the content would land.
  expect(view?.querySelector(".rail-view-loading")).toBeNull()
  expect(view?.querySelector("h1")).toBeNull()

  await act(async () => vi.advanceTimersByTime(RAIL_LOADING_DELAY_MS - 1))
  expect(view?.querySelector(".rail-view-loading")).toBeNull()

  await act(async () => vi.advanceTimersByTime(1))
  const loader = view?.querySelector(".rail-view-loading")
  expect(loader?.getAttribute("data-slot")).toBe("loading-text")
  expect(loader?.textContent).toContain("Loading Files")
  // Just the text: no panel or box drawn around it.
  expect(view?.querySelector('[data-slot="generating-surface"]')).toBeNull()

  await act(async () => root.unmount())
  host.remove()
  // Nothing left waiting once the view is gone.
  expect(vi.getTimerCount()).toBe(0)
})

it("stops waiting when the view closes before the delay", async () => {
  const host = document.createElement("div")
  const root = createRoot(host)
  await act(async () => root.render(<RailView item={files} />))
  await act(async () => root.unmount())
  expect(vi.getTimerCount()).toBe(0)
})

it("delays the loader by three seconds", () => {
  expect(RAIL_LOADING_DELAY_MS).toBe(3000)
})

it("holds the macOS traffic lights in the open rail, with the gap they keep from the window's edge past them", () => {
  const read = (path: string) => readFileSync(new URL(path, import.meta.url), "utf8")
  const px = (text: string, pattern: RegExp) => Number(pattern.exec(text)?.[1])
  // Where the host puts the close button, and the room the page leaves them.
  const left = px(
    read("../../../../../src-tauri/src/platform/macos/traffic_lights.rs"),
    /pub const LEFT: f64 = ([\d.]+);/,
  )
  const styles = read("../../../styles.css")
  const room = px(
    styles,
    /\[data-host="macos"\]\[data-surface="window"\] \{\s*--desktop-window-controls-inset: (\d+)px;/,
  )
  const gutter = px(styles, /--desktop-gutter: (\d+)px;/)
  // AppKit's three 14pt-wide buttons, 23pt apart.
  const lights = 2 * 23 + 14
  expect([left, room, gutter].every(Number.isFinite)).toBe(true)
  expect(railWidth("macos")).toBeGreaterThanOrEqual(room)
  expect(railWidth("macos") + gutter).toBe(left + lights + left)
  expect(railWidth("browser")).toBe(48)
})
