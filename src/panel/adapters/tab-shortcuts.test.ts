import { describe, expect, it } from "vitest"

import { tabShortcutAction } from "./tab-shortcuts"

function chord(
  key: string,
  mods: { meta?: boolean; ctrl?: boolean; alt?: boolean; shift?: boolean; repeat?: boolean } = {},
) {
  return {
    key,
    metaKey: Boolean(mods.meta),
    ctrlKey: Boolean(mods.ctrl),
    altKey: Boolean(mods.alt),
    shiftKey: Boolean(mods.shift),
    repeat: Boolean(mods.repeat),
  }
}

describe("tabShortcutAction", () => {
  it("opens and closes on desktop with Cmd/Ctrl and no Shift", () => {
    expect(tabShortcutAction(chord("t", { meta: true }), "desktop")).toBe("open")
    expect(tabShortcutAction(chord("n", { ctrl: true }), "desktop")).toBe("open")
    expect(tabShortcutAction(chord("w", { meta: true }), "desktop")).toBe("close")
    expect(tabShortcutAction(chord("t", { meta: true, shift: true }), "desktop")).toBeNull()
  })

  it("requires Shift on the browser preview so native browser chords stay free", () => {
    expect(tabShortcutAction(chord("t", { meta: true }), "browser")).toBeNull()
    expect(tabShortcutAction(chord("w", { ctrl: true }), "browser")).toBeNull()
    expect(tabShortcutAction(chord("t", { meta: true, shift: true }), "browser")).toBe("open")
    expect(tabShortcutAction(chord("w", { ctrl: true, shift: true }), "browser")).toBe("close")
    expect(tabShortcutAction(chord("n", { meta: true, shift: true }), "browser")).toBeNull()
  })

  it("ignores repeats, Alt, and bare keys", () => {
    expect(tabShortcutAction(chord("t", { meta: true, repeat: true }), "desktop")).toBeNull()
    expect(tabShortcutAction(chord("t", { meta: true, alt: true }), "desktop")).toBeNull()
    expect(tabShortcutAction(chord("t"), "desktop")).toBeNull()
  })
})
