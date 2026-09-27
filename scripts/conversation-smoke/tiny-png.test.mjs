import assert from "node:assert/strict"
import { test } from "node:test"
import { assertValidTinyPng, tinyPng } from "./tiny-png.mjs"

test("tiny upload fixture is a fully decodable PNG", () => {
  assertValidTinyPng(tinyPng)
})

test("tiny upload fixture validation rejects corrupted image data", () => {
  const corrupted = Buffer.from(tinyPng)
  corrupted[45] ^= 0xff
  assert.throws(() => assertValidTinyPng(corrupted))
})
