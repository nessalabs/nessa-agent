import assert from "node:assert/strict"
import { it } from "node:test"

import { css } from "./selectors.mjs"

it("can pass the complete CSS selector table to a browser evaluation", () => {
  assert.deepEqual(structuredClone(css), css)
  assert.ok(Object.values(css).every((value) => typeof value === "string"))
})
