// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, expect, it, vi } from "vitest"
import { NightScene } from "./night-scene"
import {
  nightSceneRainFar,
  nightSceneRainMid,
  nightSceneRainNear,
  nightSceneSteam,
  nightSceneWindow,
} from "../model/night-scene"

const mounted: { root: Root; host: HTMLElement }[] = []

afterEach(async () => {
  for (const { root, host } of mounted.splice(0)) {
    await act(async () => root.unmount())
    host.remove()
  }
  vi.unstubAllGlobals()
})

async function scene(still = false) {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true)
  let observed: Element | null = null
  let receiver: ResizeObserver | null = null
  let deliver: ResizeObserverCallback = () => {}
  const disconnect = vi.fn()
  class Observer implements ResizeObserver {
    constructor(callback: ResizeObserverCallback) {
      deliver = callback
    }
    observe(target: Element) {
      observed = target
    }
    unobserve(target: Element) {
      if (observed === target) observed = null
    }
    disconnect = disconnect
  }
  vi.stubGlobal("ResizeObserver", function (callback: ResizeObserverCallback) {
    const observer = new Observer(callback)
    receiver = observer
    return observer
  })
  const computedStyle = vi.fn(() => {
    throw new Error("synchronous layout read")
  })
  vi.stubGlobal("getComputedStyle", computedStyle)
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  mounted.push({ root, host })
  await act(async () => root.render(<NightScene still={still} />))
  const frame = host.querySelector<HTMLElement>(".desktop-night-scene")
  if (!frame) throw new Error("night scene did not mount")
  expect(observed).toBe(frame)
  const resize = (height: number) => {
    if (!receiver) throw new Error("night scene has no observer")
    deliver(
      [
        {
          target: frame,
          contentRect: new DOMRect(0, 0, 800, height),
          borderBoxSize: [{ inlineSize: 800, blockSize: height }],
          contentBoxSize: [{ inlineSize: 800, blockSize: height }],
          devicePixelContentBoxSize: [{ inlineSize: 800, blockSize: height }],
        },
      ],
      receiver,
    )
  }
  return { root, host, frame, resize, computedStyle, disconnect }
}

it("uses the observed content height without synchronous layout reads, and disconnects", async () => {
  const { root, host, frame, resize, computedStyle, disconnect } = await scene()
  expect(frame.hasAttribute("data-measured")).toBe(false)
  resize(290)
  expect(frame.style.getPropertyValue("--night-scene-scale")).toBe("1")
  expect(frame.style.getPropertyValue("--night-scene-em")).toBe("10px")
  expect(frame.hasAttribute("data-measured")).toBe(true)
  resize(145)
  expect(frame.style.getPropertyValue("--night-scene-scale")).toBe("0.5")
  expect(frame.style.getPropertyValue("--night-scene-em")).toBe("5px")
  expect(computedStyle).not.toHaveBeenCalled()
  await act(async () => root.unmount())
  expect(disconnect).toHaveBeenCalledOnce()
  mounted.pop()
  host.remove()
})

it("bounds a static window to its visible rain and steam, preserving animated travel", async () => {
  const animated = (await scene()).frame
  const staticFrame = (await scene(true)).frame
  for (const [depth, pattern] of [
    ["far", nightSceneRainFar],
    ["mid", nightSceneRainMid],
    ["near", nightSceneRainNear],
  ] as const) {
    const selector = `.desktop-night-scene-rain[data-depth="${depth}"] > span`
    const movingRows = animated.querySelector(selector)?.textContent?.split("\n")
    const staticRows = staticFrame.querySelector(selector)?.textContent?.split("\n")
    expect(movingRows).toEqual([...pattern, ...pattern])
    expect(staticRows).toHaveLength(nightSceneWindow.rows)
    expect(staticRows).toEqual(movingRows?.slice(0, nightSceneWindow.rows))
  }
  const steam = ".desktop-night-scene-steam > span"
  expect(animated.querySelector(steam)?.textContent?.split("\n")).toEqual([
    ...nightSceneSteam.pattern,
    ...nightSceneSteam.pattern,
  ])
  expect(staticFrame.querySelector(steam)?.textContent?.split("\n")).toEqual(
    nightSceneSteam.pattern,
  )
})
