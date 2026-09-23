// @vitest-environment jsdom

import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"

describe("embedded load fallback", () => {
  it("stays painted inside a bottom-right clipped panel", () => {
    const source = readFileSync("index.html", "utf8")
    const parsed = new DOMParser().parseFromString(source, "text/html")
    document.documentElement.innerHTML = parsed.documentElement.innerHTML
    const fallback = document.querySelector<HTMLElement>("[data-nessa-load-fallback]")
    const message = document.querySelector<HTMLElement>("[data-nessa-load-message]")

    expect(fallback).toBeInstanceOf(HTMLElement)
    expect(message).toBeInstanceOf(HTMLElement)
    if (!fallback || !message) throw new Error("embedded fallback is incomplete")

    const fallbackStyle = getComputedStyle(fallback)
    const messageStyle = getComputedStyle(message)
    expect(fallback.getAttribute("style")).toBeNull()
    expect(message.getAttribute("style")).toBeNull()
    expect(fallbackStyle.position).toBe("fixed")
    expect(fallbackStyle.inset).toBe("0px")
    expect(messageStyle.position).toBe("fixed")
    expect(messageStyle.right).toBe("0px")
    expect(messageStyle.bottom).toBe("0px")
    expect(messageStyle.maxWidth).toBe(`${innerWidth}px`)
    expect(messageStyle.maxHeight).toBe(`${innerHeight}px`)
    expect(messageStyle.overflowWrap).toBe("anywhere")

    const messageWidth = Number.parseFloat(messageStyle.width)
    const messageHeight = Number.parseFloat(messageStyle.height)
    expect(messageWidth).toBeLessThanOrEqual(320)
    expect(messageHeight).toBeLessThanOrEqual(320)

    // macOS gives WebKit an oversized stage while clipping its bottom-right
    // to the configured panel. Apply the computed box to that actual geometry.
    const stage = { width: 1440, height: 900 }
    const panel = { width: 400, height: 320 }
    const clip = {
      left: stage.width - panel.width,
      top: stage.height - panel.height,
      right: stage.width,
      bottom: stage.height,
    }
    const box = {
      left: stage.width - messageWidth,
      top: stage.height - messageHeight,
      right: stage.width,
      bottom: stage.height,
    }
    expect(box.left).toBeGreaterThanOrEqual(clip.left)
    expect(box.top).toBeGreaterThanOrEqual(clip.top)
    expect(box.right).toBeLessThanOrEqual(clip.right)
    expect(box.bottom).toBeLessThanOrEqual(clip.bottom)
  })
})
