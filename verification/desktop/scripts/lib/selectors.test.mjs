import assert from "node:assert/strict"
import { it } from "node:test"

import { chordDown, css, keys } from "./selectors.mjs"

it("can pass the complete CSS selector table to a browser evaluation", () => {
  assert.deepEqual(structuredClone(css), css)
  assert.ok(Object.values(css).every((value) => typeof value === "string"))
})

it("walks controls with the engine's platform chord", () => {
  assert.equal(keys.nextControl.chromium, "Tab")
  assert.equal(keys.previousControl.chromium, "Shift+Tab")
  assert.equal(keys.nextControl.webkit, "Alt+Tab")
  assert.equal(keys.previousControl.webkit, "Alt+Shift+Tab")
  assert.deepEqual(chordDown(keys.previousControl.webkit), {
    code: "Tab",
    altKey: true,
    ctrlKey: false,
    metaKey: false,
    shiftKey: true,
  })
  assert.deepEqual(chordDown(keys.nextControl.webkit), {
    code: "Tab",
    altKey: true,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
  })
})

it("reads the split-beside chord from the Playwright string", () => {
  assert.deepEqual(chordDown(keys.newSessionBeside), {
    code: "KeyN",
    altKey: false,
    ctrlKey: false,
    metaKey: true,
    shiftKey: true,
  })
  assert.deepEqual(chordDown(keys.newSession), {
    code: "KeyN",
    altKey: false,
    ctrlKey: false,
    metaKey: true,
    shiftKey: false,
  })
  assert.deepEqual(chordDown("KeyN"), {
    code: "KeyN",
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
  })
  assert.throws(() => chordDown("Foo+KeyN"), /not a Playwright chord/)
})
