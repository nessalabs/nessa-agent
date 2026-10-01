// @vitest-environment jsdom

import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"

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

  it.each(["", "?surface=panel", "?other=value&surface=setup", "?surface=main"])(
    "centers %s in its viewport",
    (search) => {
      const { fallback, message } = load(search)
      const fallbackStyle = getComputedStyle(fallback)
      const messageStyle = getComputedStyle(message)

      expect(fallback.getAttribute("style")).toBeNull()
      expect(message.getAttribute("style")).toBeNull()
      expect(message.textContent?.trim()).toBe("Loading")
      expect(fallbackStyle.inset).toBe("0px")
      expect(messageStyle.position).toBe("fixed")
      expect(messageStyle.inset).toBe("0px")
      expect(messageStyle.width).toBe("auto")
      expect(messageStyle.height).toBe("auto")
      expect(messageStyle.display).toBe("grid")
      expect(messageStyle.placeContent).toBe("center")
      expect(messageStyle.maxWidth).toBe(`${innerWidth}px`)
      expect(messageStyle.maxHeight).toBe(`${innerHeight}px`)
    },
  )
})
