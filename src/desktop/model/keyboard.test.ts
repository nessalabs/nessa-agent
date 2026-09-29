import { describe, expect, it } from "vitest"
import {
  chordLabel,
  commandLabel,
  macUserAgent,
  matchesChord,
  type Chord,
} from "./keyboard"

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

  it("writes Option as ⌥ on a Mac and as Alt everywhere else", () => {
    const panel: Chord = { code: "KeyB", command: true, alt: true }
    expect(chordLabel(panel, true)).toBe("⌥⌘B")
    expect(chordLabel(panel, false)).toBe("Ctrl+Alt+B")
    expect(chordLabel(panel, false)).not.toContain("⌥")
  })

  it("writes the command key before a click as the platform does", () => {
    expect(commandLabel(true)).toBe("⌘")
    expect(commandLabel(false)).toBe("Ctrl+")
  })
})

describe("reading the platform", () => {
  it("is a Mac by a Mac's user agent, and not otherwise", () => {
    expect(
      macUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 15_0) AppleWebKit/605.1.15"),
    ).toBe(true)
    expect(macUserAgent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")).toBe(false)
    expect(macUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe(false)
    expect(macUserAgent("")).toBe(false)
  })
})
