import { describe, expect, it } from "vitest"
import { chordLabel, labelOf, matchesChord, type Chord } from "./shortcuts"

const key = (
  code: string,
  modifiers: Partial<Record<"metaKey" | "ctrlKey" | "shiftKey" | "altKey", boolean>> = {},
) => ({
  code,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  ...modifiers,
})

const splitDown: Chord = { code: "Backslash", command: true, shift: true }
const moveLeft: Chord = { code: "ArrowLeft", control: true, alt: true }

describe("matching chords", () => {
  it("takes ⌘ as the command key on a Mac, exactly", () => {
    expect(
      matchesChord(key("Backslash", { metaKey: true, shiftKey: true }), splitDown, true),
    ).toBe(true)
    expect(matchesChord(key("Backslash", { metaKey: true }), splitDown, true)).toBe(false)
    expect(
      matchesChord(key("Backslash", { ctrlKey: true, shiftKey: true }), splitDown, true),
    ).toBe(false)
    expect(
      matchesChord(key("ArrowLeft", { ctrlKey: true, altKey: true }), moveLeft, true),
    ).toBe(true)
    expect(
      matchesChord(key("ArrowLeft", { metaKey: true, altKey: true }), moveLeft, true),
    ).toBe(false)
  })

  it("takes Control as the command key elsewhere", () => {
    expect(
      matchesChord(key("Backslash", { ctrlKey: true, shiftKey: true }), splitDown, false),
    ).toBe(true)
    expect(
      matchesChord(key("Backslash", { metaKey: true, shiftKey: true }), splitDown, false),
    ).toBe(false)
    expect(
      matchesChord(key("ArrowLeft", { ctrlKey: true, altKey: true }), moveLeft, false),
    ).toBe(true)
  })

  it("matches nothing on another key", () => {
    expect(
      matchesChord(key("KeyB", { metaKey: true }), { code: "KeyN", command: true }, true),
    ).toBe(false)
  })
})

describe("writing chords", () => {
  it("writes a Mac's symbols in their order, and names elsewhere", () => {
    expect(chordLabel(splitDown, true)).toBe("⇧⌘\\")
    expect(chordLabel({ code: "KeyS", command: true, alt: true }, true)).toBe("⌥⌘S")
    expect(chordLabel(moveLeft, true)).toBe("⌃⌥←")
    expect(chordLabel(splitDown, false)).toBe("Ctrl+Shift+\\")
    expect(chordLabel(moveLeft, false)).toBe("Ctrl+Alt+←")
    expect(chordLabel({ code: "Digit1", command: true }, true)).toBe("⌘1")
  })

  it("labels a command by the first chord bound to it", () => {
    const bindings = [
      { chord: { code: "KeyK", command: true }, command: "search" },
      { chord: { code: "KeyF", command: true }, command: "search" },
    ] as const
    expect(labelOf(bindings, "search")?.endsWith("K")).toBe(true)
    expect(labelOf(bindings, "other" as "search")).toBeUndefined()
  })
})
