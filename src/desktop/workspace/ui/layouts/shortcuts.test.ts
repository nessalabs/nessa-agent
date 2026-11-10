import { describe, expect, it } from "vitest"
import { chordEvent, matchesChord } from "../../../model/keyboard"
import { workspaceShortcuts } from "./shortcuts"

describe("the window's keyboard", () => {
  for (const mac of [true, false]) {
    it(`gives every command its own keys (${mac ? "Mac" : "elsewhere"})`, () => {
      for (const binding of workspaceShortcuts) {
        const event = chordEvent(binding.chord, mac)
        const answering = workspaceShortcuts.filter((other) =>
          matchesChord(event, other.chord, mac),
        )
        expect(answering.map((other) => other.command)).toEqual([binding.command])
      }
    })
  }

})
