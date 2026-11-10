// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, expect, it, vi } from "vitest"
import {
  ambientBlurPadPx,
  ambientBlurPx,
  ambientScale,
  DesktopAmbient,
  paintAmbientLight,
  transparentStop,
} from "./ambient-light"

const mounted: { root: Root; host: HTMLElement }[] = []

afterEach(async () => {
  for (const { root, host } of mounted.splice(0)) {
    await act(async () => root.unmount())
    host.remove()
  }
  vi.unstubAllGlobals()
})

it("scales the texture to the circle plus the blur's spread", () => {
  expect(ambientBlurPadPx).toBe(ambientBlurPx * 3)
  expect(ambientScale(1008, 192)).toBe((1008 + ambientBlurPadPx * 2) / 192)
})

it("fades a colour out by clearing its own alpha", () => {
  expect(transparentStop("oklch(0.78 0.005 250 / 16%)")).toBe("oklch(0.78 0.005 250 / 0)")
  expect(transparentStop("rgb(10, 20, 30)")).toBe("rgba(0,0,0,0)")
})

it("blurs the small texture by the on-screen blur divided by the scale", () => {
  const seen: string[] = []
  const canvas = {
    width: 0,
    height: 0,
    getContext: () => ({
      clearRect() {},
      beginPath() {},
      arc() {},
      fill() {},
      createRadialGradient: () => ({ addColorStop() {} }),
      set filter(value: string) {
        if (value) seen.push(value)
      },
      get filter() {
        return ""
      },
      fillStyle: "",
    }),
    toDataURL: () => "data:image/png;base64,e30=",
  } as unknown as HTMLCanvasElement
  const url = paintAmbientLight(canvas, 1008, 192, "oklch(0.78 0.005 250 / 16%)")
  const scale = ambientScale(1008, 192)
  expect(url).toMatch(/^data:image\/png/)
  expect(seen).toContain(`blur(${ambientBlurPx / scale}px)`)
  expect(canvas.width).toBe(192)
})

it("draws the two lights and the grain", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true)
  class Observer implements ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  vi.stubGlobal("ResizeObserver", Observer)
  const host = document.createElement("div")
  host.setAttribute("data-desktop-theme", "graphite")
  document.body.appendChild(host)
  const root = createRoot(host)
  mounted.push({ root, host })
  await act(async () => root.render(<DesktopAmbient />))
  expect(host.querySelectorAll(".desktop-ambient-light")).toHaveLength(2)
  expect(host.querySelector(".desktop-grain")).not.toBeNull()
  expect(host.querySelector(".desktop-ambient-light-low")).not.toBeNull()
})
