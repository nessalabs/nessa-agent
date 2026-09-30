import { describe, expect, it } from "vitest"
import { generatedStrings } from "../../model/testing"
import {
  paneItemKey,
  paneItemOf,
  sessionItem,
  widgetItem,
  type PaneItem,
} from "./pane-item"

const strings = generatedStrings(1500, 0x327)
/** Sessions and widgets over the same strings, so a session id can look like a widget's key. */
const items: PaneItem[] = strings.flatMap((text, at) => [
  sessionItem(text),
  widgetItem({ plugin: text, id: strings[(at * 7 + 3) % strings.length] }),
  widgetItem({ plugin: strings[(at * 11 + 5) % strings.length], id: text }),
])
/** One string per item value, for telling equal items from different ones. */
const valueOf = (item: PaneItem) => JSON.stringify(item)

describe("a pane item's key", () => {
  it("names a session by its id and a widget by its plugin and id, encoded", () => {
    expect(paneItemKey(sessionItem("retry"))).toBe("s:retry")
    expect(paneItemKey(widgetItem({ plugin: "experiments", id: "run:1" }))).toBe(
      "w:experiments:run%003A1",
    )
    expect(paneItemKey(widgetItem({ plugin: "", id: "" }))).toBe("w::")
  })

  it("keeps apart what the prototype's key let collide", () => {
    // A session whose id reads as a widget's key, and a widget id with a colon.
    const session = sessionItem("w:experiments:run")
    const widget = widgetItem({ plugin: "experiments", id: "run" })
    expect(paneItemKey(session)).not.toBe(paneItemKey(widget))
    expect(paneItemKey(widgetItem({ plugin: "a:b", id: "c" }))).not.toBe(
      paneItemKey(widgetItem({ plugin: "a", id: "b:c" })),
    )
    expect(paneItemOf(paneItemKey(session))).toEqual(session)
  })

  it("reads back every item it writes, a lone surrogate among them", () => {
    const lone = widgetItem({ plugin: "\uD800", id: "\uDFFF" })
    expect(paneItemOf(paneItemKey(lone))).toEqual(lone)
    for (const item of items)
      expect(paneItemOf(paneItemKey(item)), valueOf(item)).toEqual(item)
  })

  it("is one-to-one: two items share a key exactly when they are equal", () => {
    const byKey = new Map<string, string>()
    for (const item of items) {
      const key = paneItemKey(item)
      const before = byKey.get(key)
      if (before !== undefined) expect(before, key).toBe(valueOf(item))
      byKey.set(key, valueOf(item))
    }
    expect(byKey.size).toBe(new Set(items.map(valueOf)).size)
  })

  it("is canonical: what reads as an item is exactly the key that item has", () => {
    const keys = items.map(paneItemKey)
    // Keys, keys bent out of shape, and strings that were never keys.
    const candidates = [
      ...keys,
      ...keys.map((key) => `${key}:`),
      ...keys.map((key) => key.toLowerCase()),
      ...strings.map((text) => `w:${text}`),
      ...strings,
    ]
    let read = 0
    for (const candidate of candidates) {
      const item = paneItemOf(candidate)
      if (item === null) continue
      read++
      expect(paneItemKey(item), JSON.stringify(candidate)).toBe(candidate)
    }
    expect(read).toBeGreaterThanOrEqual(keys.length)
    expect(read).toBeLessThan(candidates.length)
  })

  it("reads nothing from a string it did not write", () => {
    for (const stray of [
      "",
      "retry",
      "x:retry",
      "w:a",
      "w:a:b:c",
      "w:%0041:b",
      "w:a b:c",
    ])
      expect(paneItemOf(stray), stray).toBeNull()
  })
})
