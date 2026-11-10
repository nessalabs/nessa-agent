// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, expect, it, vi } from "vitest"
import { ClockProvider } from "../../clock"
import {
  ambientBlurPadPx,
  ambientBlurPx,
  ambientScale,
  DesktopAmbient,
  isAmbientLightTransition,
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

it("follows only a light-colour transition on the surface itself", () => {
  const surface = document.createElement("div")
  const control = document.createElement("button")
  surface.append(control)
  expect(
    isAmbientLightTransition({ target: control, propertyName: "background" }, surface),
  ).toBe(false)
  expect(
    isAmbientLightTransition({ target: surface, propertyName: "background" }, surface),
  ).toBe(false)
  expect(
    isAmbientLightTransition(
      { target: surface, propertyName: "--desktop-light-high" },
      surface,
    ),
  ).toBe(true)
})

it("keeps one repaint frame when the light colour changes, and none for a hover", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true)
  class Observer implements ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  vi.stubGlobal("ResizeObserver", Observer)
  const frames: FrameRequestCallback[] = []
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    frames.push(callback)
    return frames.length
  })
  vi.stubGlobal("cancelAnimationFrame", () => {})
  // A seeded workspace freezes its clock. The repaint deadline must not use it.
  const workspaceNow = () => 1_700_000_000_000
  let elapsed = 1_000

  const surface = document.createElement("div")
  surface.setAttribute("data-desktop-theme", "graphite")
  const mountPoint = document.createElement("div")
  const control = document.createElement("button")
  surface.append(mountPoint, control)
  document.body.appendChild(surface)
  const root = createRoot(mountPoint)
  mounted.push({ root, host: surface })
  await act(async () =>
    root.render(
      <ClockProvider now={workspaceNow} elapsed={() => elapsed}>
        <DesktopAmbient />
      </ClockProvider>,
    ),
  )

  const burst = (target: EventTarget, propertyName: string) => {
    for (let i = 0; i < 100; i += 1) {
      target.dispatchEvent(
        new TransitionEvent("transitionrun", { bubbles: true, propertyName }),
      )
    }
  }
  burst(control, "background")
  burst(surface, "background")
  expect(frames).toHaveLength(0)
  burst(surface, "--desktop-light-high")
  expect(frames).toHaveLength(1)

  elapsed = 2_000
  frames[0]?.(0)
  expect(frames).toHaveLength(1)
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
  await act(async () =>
    root.render(
      <ClockProvider now={() => 0} elapsed={() => 0}>
        <DesktopAmbient />
      </ClockProvider>,
    ),
  )
  expect(host.querySelectorAll(".desktop-ambient-light")).toHaveLength(2)
  expect(host.querySelector(".desktop-grain")).not.toBeNull()
  expect(host.querySelector(".desktop-ambient-light-low")).not.toBeNull()
})
