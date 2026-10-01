// @vitest-environment jsdom

import { existsSync, readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"

// jsdom resolves no `var()` and lays nothing out, so where the fallback lands
// on a stage larger than the window is measured in real WebKit and Chromium by
// verification/desktop/scripts/load-fallback.mjs. These tests hold the rules
// that decide it: which surface the page is, and what each surface declares.

describe("embedded load fallback", () => {
  function load(search: string) {
    const source = readFileSync("index.html", "utf8")
    const parsed = new DOMParser().parseFromString(source, "text/html")
    document.documentElement.innerHTML = parsed.documentElement.innerHTML
    document.documentElement.dataset.nessaSurface =
      parsed.documentElement.dataset.nessaSurface
    window.history.replaceState({}, "", `/${search}`)
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

  it("keeps the panel's message in a bottom-right box the window size replaces", () => {
    const { message } = load("")
    const style = getComputedStyle(message)
    expect(style.position).toBe("fixed")
    expect(style.right).toBe("0px")
    expect(style.bottom).toBe("0px")
    expect(style.width).toBe("var(--nessa-window-width, 320px)")
    expect(style.height).toBe("var(--nessa-window-height, 320px)")
    expect(style.display).toBe("grid")
    expect(style.placeContent).toBe("center")
  })

  it("centers setup in its ordinary viewport", () => {
    const { message } = load("?other=value&surface=setup")
    expect(document.documentElement.dataset.nessaSurface).toBe("setup")

    const style = getComputedStyle(message)
    expect(style.inset).toBe("0px")
    expect(style.width).toBe("auto")
    expect(style.height).toBe("auto")
    expect(style.placeContent).toBe("center")
  })
})
