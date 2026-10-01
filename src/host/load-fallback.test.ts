// @vitest-environment jsdom

import { existsSync, readFileSync } from "node:fs"
import { afterEach, describe, expect, it, vi } from "vitest"

// jsdom resolves no `var()` and lays nothing out, so where the fallback lands
// on a stage larger than the window is measured in real WebKit and Chromium by
// verification/desktop/scripts/load-fallback.mjs. These tests hold the rules
// that decide it: which surface the page is, and the window size it is given.

type Internals = { invoke: (command: string) => Promise<unknown> }

declare global {
  interface Window {
    __TAURI_INTERNALS__?: Internals
  }
}

describe("embedded load fallback", () => {
  afterEach(() => {
    delete window.__TAURI_INTERNALS__
    document.documentElement.removeAttribute("style")
  })

  function load(search: string, internals?: Internals) {
    const source = readFileSync("index.html", "utf8")
    const parsed = new DOMParser().parseFromString(source, "text/html")
    document.documentElement.removeAttribute("style")
    document.documentElement.innerHTML = parsed.documentElement.innerHTML
    document.documentElement.dataset.nessaSurface =
      parsed.documentElement.dataset.nessaSurface
    window.history.replaceState({}, "", `/${search}`)
    if (internals) window.__TAURI_INTERNALS__ = internals
    const bootstrap = document.querySelector<HTMLScriptElement>(
      "script[data-nessa-load-surface]",
    )
    const application = document.querySelector<HTMLScriptElement>("script[type=module]")
    expect(bootstrap?.textContent).toBeTruthy()
    expect(bootstrap?.hasAttribute("src")).toBe(false)
    expect(application).toBeInstanceOf(HTMLScriptElement)
    expect(
      bootstrap && application
        ? bootstrap.compareDocumentPosition(application) &
            Node.DOCUMENT_POSITION_FOLLOWING
        : 0,
    ).not.toBe(0)
    window.eval(bootstrap?.textContent ?? "")

    const fallback = document.querySelector<HTMLElement>("[data-nessa-load-fallback]")
    const message = document.querySelector<HTMLElement>("[data-nessa-load-message]")

    expect(fallback).toBeInstanceOf(HTMLElement)
    expect(message).toBeInstanceOf(HTMLElement)
    if (!fallback || !message) throw new Error("embedded fallback is incomplete")

    return { fallback, message }
  }

  const windowSize = () => {
    const style = document.documentElement.style
    return {
      width: style.getPropertyValue("--nessa-window-width"),
      height: style.getPropertyValue("--nessa-window-height"),
    }
  }

  const host = (answer: () => Promise<unknown>) => {
    const invoke = vi.fn((command: string) =>
      command === "panel_size" ? answer() : Promise.reject(new Error(command)),
    )
    return { invoke }
  }

  it.each(["", "?surface=panel", "?surface=anything-else"])(
    "keeps %s on the panel fallback",
    (search) => {
      load(search)
      expect(document.documentElement.dataset.nessaSurface).toBe("panel")
    },
  )

  it("shows the agent's avatar and only the word Loading, as a status", () => {
    const { message } = load("")
    expect(message.getAttribute("role")).toBe("status")
    expect(message.textContent?.trim()).toBe("Loading")
    const avatar = message.querySelector<HTMLImageElement>("img[data-nessa-load-mark]")
    expect(avatar?.getAttribute("alt")).toBe("")
    // The one stored avatar, not a copy of it.
    const src = avatar?.getAttribute("src") ?? ""
    expect(src).toBe("/src-tauri/icons/nessa-avatar.svg")
    expect(existsSync(src.slice(1))).toBe(true)
  })

  it("sizes the panel's message to the window the host reports", async () => {
    const tauri = host(() => Promise.resolve({ width: 420, height: 835 }))
    const { message } = load("", tauri)
    await vi.waitFor(() =>
      expect(windowSize()).toEqual({ width: "420px", height: "835px" }),
    )
    expect(tauri.invoke).toHaveBeenCalledWith("panel_size")

    const style = getComputedStyle(message)
    expect(style.position).toBe("fixed")
    expect(style.right).toBe("0px")
    expect(style.bottom).toBe("0px")
    expect(style.width).toBe("var(--nessa-window-width, 320px)")
    expect(style.height).toBe("var(--nessa-window-height, 320px)")
    expect(style.display).toBe("grid")
    expect(style.placeContent).toBe("center")
  })

  it.each([
    ["never answers", () => new Promise(() => {})],
    ["refuses", () => Promise.reject(new Error("refused"))],
    ["answers nothing", () => Promise.resolve(null)],
    ["answers a zero size", () => Promise.resolve({ width: 0, height: 835 })],
    [
      "answers a size that is not a length",
      () => Promise.resolve({ width: Infinity, height: 835 }),
    ],
  ])("keeps the panel's smallest box when the host %s", async (_, answer) => {
    const tauri = host(answer)
    load("", tauri)
    await new Promise((resolve) => setTimeout(resolve))
    expect(tauri.invoke).toHaveBeenCalledWith("panel_size")
    expect(windowSize()).toEqual({ width: "", height: "" })
  })

  it("treats a plain browser's viewport as the window", () => {
    load("")
    expect(windowSize()).toEqual({ width: "100vw", height: "100vh" })
  })

  it("centers setup in its ordinary viewport without asking for the panel's size", () => {
    const tauri = host(() => Promise.resolve({ width: 420, height: 835 }))
    const { message } = load("?other=value&surface=setup", tauri)
    expect(document.documentElement.dataset.nessaSurface).toBe("setup")
    expect(tauri.invoke).not.toHaveBeenCalled()
    expect(windowSize()).toEqual({ width: "", height: "" })

    const style = getComputedStyle(message)
    expect(style.inset).toBe("0px")
    expect(style.width).toBe("auto")
    expect(style.height).toBe("auto")
    expect(style.placeContent).toBe("center")
  })
})
