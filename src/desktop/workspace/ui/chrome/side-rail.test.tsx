// @vitest-environment jsdom
import { act } from "react"
import { createRoot } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import {
  RAIL_LOADING_DELAY_MS,
  RailContentProvider,
  RailView,
  railItems,
  SIDE_RAIL_WIDTH,
  type RailItem,
} from "./side-rail"

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

/** An item the composition gave no content. */
const files: RailItem = { id: "files", name: "Files", icon: <svg /> }

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

it("fits the compact rail below the window bar on every host", () => {
  expect(SIDE_RAIL_WIDTH).toBe(64)
})

it("lists Agents and Devices only", () => {
  expect(railItems.map((item) => item.id)).toEqual(["agents", "devices"])
})

it('keeps "nessa Studio" in the corner for Agents but not for Devices', () => {
  expect(railItems.map((item) => [item.id, item.studio !== false])).toEqual([
    ["agents", true],
    ["devices", false],
  ])
})

it("shows the content the composition gave an item, with no loader", async () => {
  const host = document.createElement("div")
  const root = createRoot(host)
  const devices = railItems.find((item) => item.id === "devices")
  if (!devices) throw new Error("the rail has no Devices item")
  await act(async () =>
    root.render(
      <RailContentProvider views={{ devices: <p data-content>Devices page</p> }}>
        <RailView item={devices} />
      </RailContentProvider>,
    ),
  )
  expect(host.querySelector("[data-content]")?.textContent).toBe("Devices page")
  await act(async () => vi.advanceTimersByTime(RAIL_LOADING_DELAY_MS))
  expect(host.querySelector(".rail-view-loading")).toBeNull()
  await act(async () => root.unmount())
  expect(vi.getTimerCount()).toBe(0)
})
