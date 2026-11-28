// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { afterEach, expect, it, vi } from "vitest"
import { chordEvent } from "../model/keyboard"
import { labelOf } from "./key-bindings"
import {
  bindWindowZoom,
  useWindowZoomInEffect,
  zoomAvailable,
  zoomShortcuts,
} from "./window-zoom"

vi.mock("./platform", () => ({ isMac: true }))

const KEY = "nessa.desktop.zoom"

async function mounted() {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  function Window() {
    useWindowZoomInEffect()
    return null
  }
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  await React.act(async () => root.render(React.createElement(Window)))
  return async () => {
    await React.act(async () => root.unmount())
    host.remove()
  }
}

/** Presses a chord on the window, and says whether the window took the key. */
async function press(chord: (typeof zoomShortcuts)[number]["chord"]): Promise<boolean> {
  const event = new KeyboardEvent("keydown", {
    ...chordEvent(chord, true),
    cancelable: true,
  })
  await React.act(async () => {
    window.dispatchEvent(event)
  })
  return event.defaultPrevented
}

const plus = { code: "Equal", command: true } as const
const minus = { code: "Minus", command: true } as const
const zero = { code: "Digit0", command: true } as const

afterEach(() => {
  bindWindowZoom(null)
  window.localStorage.clear()
  document.documentElement.style.removeProperty("--desktop-zoom")
  delete document.documentElement.dataset.zoom
})

it("asks the host for the remembered zoom at mount, and says it on the root once applied", async () => {
  window.localStorage.setItem(KEY, "1.25")
  const apply = vi.fn((_zoom: number) => Promise.resolve())
  bindWindowZoom({ apply })
  const close = await mounted()
  try {
    expect(apply.mock.calls).toEqual([[1.25]])
    expect(document.documentElement.style.getPropertyValue("--desktop-zoom")).toBe("1.25")
    expect(document.documentElement.dataset.zoom).toBe("1.25")
  } finally {
    await close()
  }
})

it("steps with ⌘=, ⇧⌘= and ⌘-, stops at each end, and goes back to 100% on ⌘0", async () => {
  const apply = vi.fn((_zoom: number) => Promise.resolve())
  bindWindowZoom({ apply })
  const close = await mounted()
  try {
    expect(await press(plus)).toBe(true)
    expect(await press({ ...plus, shift: true })).toBe(true)
    expect(window.localStorage.getItem(KEY)).toBe("1.25")
    for (let i = 0; i < 4; i += 1) await press(plus)
    expect(window.localStorage.getItem(KEY)).toBe("1.5")
    expect(await press(zero)).toBe(true)
    expect(window.localStorage.getItem(KEY)).toBe("1")
    for (let i = 0; i < 5; i += 1) expect(await press(minus)).toBe(true)
    expect(window.localStorage.getItem(KEY)).toBe("0.8")
    // The host was asked for each zoom the window came to, in order, and
    // the root says the last.
    expect(apply.mock.calls.map(([zoom]) => zoom)).toEqual([
      1, 1.1, 1.25, 1.5, 1, 0.9, 0.8,
    ])
    expect(document.documentElement.dataset.zoom).toBe("0.8")
  } finally {
    await close()
  }
})

it("says on the root only what the host did: a refused zoom leaves it as it was", async () => {
  const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
  const apply = vi.fn((zoom: number) =>
    zoom === 1.1 ? Promise.reject(new Error("refused")) : Promise.resolve(),
  )
  bindWindowZoom({ apply })
  const close = await mounted()
  try {
    await press(plus)
    expect(apply).toHaveBeenLastCalledWith(1.1)
    expect(document.documentElement.dataset.zoom).toBe("1")
    expect(warn).toHaveBeenCalledWith("The window did not zoom", expect.any(Error))
  } finally {
    await close()
    warn.mockRestore()
  }
})

it("says on the root only the newest ask, whichever the host finishes last", async () => {
  const asked: { zoom: number; done: () => void }[] = []
  bindWindowZoom({
    apply: (zoom) => new Promise<void>((done) => asked.push({ zoom, done })),
  })
  const close = await mounted()
  try {
    await press(plus)
    await press(plus)
    expect(asked.map((ask) => ask.zoom)).toEqual([1, 1.1, 1.25])
    // The newest is answered first, then the ones before it.
    for (const ask of [...asked].reverse()) await React.act(async () => ask.done())
    expect(document.documentElement.dataset.zoom).toBe("1.25")
  } finally {
    await close()
  }
})

it("leaves the keys to the browser where no host zooms the window", async () => {
  expect(zoomAvailable()).toBe(false)
  const close = await mounted()
  try {
    expect(await press(plus)).toBe(false)
    expect(await press(minus)).toBe(false)
    expect(await press(zero)).toBe(false)
    expect(window.localStorage.getItem(KEY)).toBeNull()
    expect(document.documentElement.dataset.zoom).toBeUndefined()
  } finally {
    await close()
  }
})

it("labels each command by its first chord", () => {
  expect(labelOf(zoomShortcuts, "zoomIn")).toBe("⌘+")
  expect(labelOf(zoomShortcuts, "zoomOut")).toBe("⌘-")
  expect(labelOf(zoomShortcuts, "zoomReset")).toBe("⌘0")
})
