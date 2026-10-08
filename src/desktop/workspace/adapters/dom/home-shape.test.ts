// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest"
import { settleOnReshape } from "./home-shape"

afterEach(() => vi.unstubAllGlobals())

it("takes its first shape from observation without settling a newly shown home", () => {
  let observed: Element | undefined
  let resized = () => {}
  const disconnected = vi.fn()
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        resized = callback
      }
      observe(target: Element) {
        observed = target
      }
      disconnect = disconnected
    },
  )
  const home = document.createElement("div")
  home.style.setProperty("--workspace-home-settle", "workspace-home-card")
  const read = vi.fn(() => home.style)
  vi.stubGlobal("getComputedStyle", read)
  const stop = settleOnReshape(home)
  expect(observed).toBe(home)
  expect(read).not.toHaveBeenCalled()
  expect(home.hasAttribute("data-reshaped")).toBe(false)
  // A size change before the first paint is still this home's first shape.
  home.style.setProperty("--workspace-home-settle", "workspace-home-page")
  resized()
  expect(read).toHaveBeenCalledOnce()
  expect(home.hasAttribute("data-reshaped")).toBe(false)
  resized()
  expect(home.hasAttribute("data-reshaped")).toBe(false)
  home.style.setProperty("--workspace-home-settle", "workspace-home-card")
  resized()
  expect(home.hasAttribute("data-reshaped")).toBe(true)
  stop()
  expect(disconnected).toHaveBeenCalledOnce()
})
