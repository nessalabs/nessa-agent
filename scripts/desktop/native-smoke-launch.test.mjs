import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { JSDOM } from "jsdom"
import test from "node:test"

test("the embedded document explains a frontend that never replaces it", () => {
  const document = readFileSync("index.html", "utf8")

  assert.match(document, /data-nessa-load-fallback/)
  assert.match(document, /Loading Nessa/)
  assert.match(document, /If this stays on screen/)
})

test("the fallback stays painted inside a bottom-right clipped panel", () => {
  const document = readFileSync("index.html", "utf8")
  const dom = new JSDOM(document)
  const fallback = dom.window.document.querySelector("[data-nessa-load-fallback]")
  const message = dom.window.document.querySelector("[data-nessa-load-message]")
  const fallbackStyle = dom.window.getComputedStyle(fallback)
  const messageStyle = dom.window.getComputedStyle(message)

  assert.equal(fallback.getAttribute("style"), null)
  assert.equal(message.getAttribute("style"), null)
  assert.equal(fallbackStyle.position, "fixed")
  assert.equal(fallbackStyle.inset, "0px")
  assert.equal(messageStyle.position, "fixed")
  assert.equal(messageStyle.right, "0px")
  assert.equal(messageStyle.bottom, "0px")
  assert.equal(messageStyle.maxWidth, `${dom.window.innerWidth}px`)
  assert.equal(messageStyle.maxHeight, `${dom.window.innerHeight}px`)
  assert.equal(messageStyle.overflowWrap, "anywhere")

  const messageWidth = Number.parseFloat(messageStyle.width)
  const messageHeight = Number.parseFloat(messageStyle.height)
  assert.ok(messageWidth <= 320)
  assert.ok(messageHeight <= 320)

  // macOS gives WebKit an oversized stage while clipping its bottom-right to
  // the configured panel. The actual CSS box must fall wholly in that clip.
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
  assert.ok(box.left >= clip.left)
  assert.ok(box.top >= clip.top)
  assert.ok(box.right <= clip.right)
  assert.ok(box.bottom <= clip.bottom)
})
