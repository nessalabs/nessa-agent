// @vitest-environment jsdom

import { existsSync, readFileSync } from "node:fs"
import { afterEach, describe, expect, it, vi } from "vitest"
import { embedLoadFallback } from "./load-fallback.mjs"

// jsdom resolves no `var()` and lays nothing out, so where the fallback lands
// on a stage larger than the window is measured in real WebKit and Chromium by
// verification/desktop/scripts/load-fallback.mjs. These tests hold the rules
// that decide it: which surface the page is, and what each surface declares.

describe("embedded load fallback", () => {
  function load(search: string) {
    const source = embedLoadFallback(readFileSync("index.html", "utf8"))
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

describe("a page the dev server did not serve", () => {
  afterEach(() => {
    vi.useRealTimers()
    delete document.documentElement.dataset.nessaModule
    delete document.documentElement.dataset.nessaMounted
  })

  function boot(html = readFileSync("index.html", "utf8")) {
    vi.useFakeTimers()
    const source = embedLoadFallback(html)
    const parsed = new DOMParser().parseFromString(source, "text/html")
    document.documentElement.innerHTML = parsed.documentElement.innerHTML
    if (parsed.documentElement.dataset.nessaSurface)
      document.documentElement.dataset.nessaSurface =
        parsed.documentElement.dataset.nessaSurface
    else delete document.documentElement.dataset.nessaSurface
    const bootstrap = document.querySelector<HTMLScriptElement>(
      "script[data-nessa-load-bootstrap]",
    )
    expect(bootstrap?.hasAttribute("src")).toBe(false)
    expect(bootstrap?.textContent).toContain("did not serve")
    window.eval(bootstrap?.textContent ?? "")
    const title = document.querySelector<HTMLElement>("[data-nessa-load-title]")
    if (!title) throw new Error("missing load title")
    return { title }
  }

  it("names the page and the script when the module is not served", () => {
    const { title } = boot()
    const script = document.querySelector<HTMLScriptElement>("script[type=module]")
    script?.dispatchEvent(new Event("error"))
    expect(title.textContent).toContain(window.location.href)
    expect(title.textContent).toContain("/src/main.tsx")
    expect(title.textContent).toContain("did not serve")
  })

  it("names a module tag that is parsed after the bootstrap runs", () => {
    vi.useFakeTimers()
    const source = embedLoadFallback(readFileSync("index.html", "utf8"))
    const parsed = new DOMParser().parseFromString(source, "text/html")
    document.documentElement.innerHTML = parsed.documentElement.innerHTML
    const script = document.querySelector<HTMLScriptElement>("script[type=module]")
    script?.remove()
    const bootstrap = document.querySelector<HTMLScriptElement>(
      "script[data-nessa-load-bootstrap]",
    )
    window.eval(bootstrap?.textContent ?? "")
    if (!script) throw new Error("missing module script")
    document.body.append(script)
    script.dispatchEvent(new Event("error"))
    const title = document.querySelector("[data-nessa-load-title]")
    expect(title?.textContent).toContain("/src/main.tsx")
    expect(title?.textContent).toContain("did not serve")
  })

  it("names the app module, not an earlier inline module, while it is still compiling", () => {
    vi.useFakeTimers()
    const { title } = boot()
    const inline = document.createElement("script")
    inline.type = "module"
    inline.textContent = "inline"
    const client = document.createElement("script")
    client.type = "module"
    client.src = "/@vite/client"
    const app = document.querySelector("script[type=module]")
    app?.before(inline, client)
    vi.advanceTimersByTime(15_000)
    expect(title.textContent).toContain("/src/main.tsx")
    expect(title.textContent).not.toContain("@vite/client")
  })

  it("says the dev server may still be compiling when the module has not started", () => {
    vi.useFakeTimers()
    const { title } = boot()
    vi.advanceTimersByTime(15_000)
    expect(title.textContent).toContain("compiling")
    expect(title.textContent).toContain("/src/main.tsx")
  })

  it("leaves Loading once the module has started", () => {
    vi.useFakeTimers()
    document.documentElement.dataset.nessaModule = "started"
    const { title } = boot()
    vi.advanceTimersByTime(15_000)
    expect(title.textContent).toBe("Loading")
  })

  it("names a runtime error that happens before the page mounts", () => {
    const { title } = boot()
    window.dispatchEvent(new ErrorEvent("error", { message: "boom" }))
    expect(title.textContent).toContain("Could not load")
    expect(title.textContent).toContain("boom")
  })

  it("fills the desktop window, which has no surface", () => {
    const { title } = boot(readFileSync("desktop.html", "utf8"))
    const message = title.parentElement
    if (!message) throw new Error("missing load message")
    const style = getComputedStyle(message)
    expect(style.inset).toBe("0px")
  })
})
