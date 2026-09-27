import { describe, expect, it } from "vitest"
import { chordEvent, matchesChord } from "../../adapters/dom/shortcuts"
import { sessionsInSidebarShortcuts, threeColumnsShortcuts } from "./shortcuts"

describe("a layout's keyboard", () => {
  for (const [layout, bindings] of [
    ["three columns", threeColumnsShortcuts],
    ["sessions in the sidebar", sessionsInSidebarShortcuts],
  ] as const) {
    for (const mac of [true, false]) {
      it(`gives every command its own keys in ${layout} (${mac ? "Mac" : "elsewhere"})`, () => {
        for (const binding of bindings) {
          const event = chordEvent(binding.chord, mac)
          const answering = bindings.filter((other) =>
            matchesChord(event, other.chord, mac),
          )
          expect(answering.map((other) => other.command)).toEqual([binding.command])
        }
      })
    }
  }
})
