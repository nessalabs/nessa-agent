/** Bare Node, no `node_modules`, like the reader it tests. */
import assert from "node:assert/strict"
import test from "node:test"

import { importedPaths } from "./imported-paths.mjs"

test("every way a module is reached from text is an import", () => {
  const text = [
    'import { a } from "./a"',
    'import type { B } from "../b"',
    'export * from "./c"',
    'export { d } from "./d"',
    'import "./e.css"',
    'const f = await import("./f")',
    'vi.mock("./g", () => ({}))',
    'vi.doMock("./h")',
  ].join("\n")
  assert.deepEqual(importedPaths(text).sort(), [
    "../b",
    "./a",
    "./c",
    "./d",
    "./e.css",
    "./f",
    "./g",
    "./h",
  ])
})

test("a string that only names a path is not an import", () => {
  for (const text of [
    'const sheet = readFileSync(new URL("./x.css", import.meta.url), "utf8")',
    'const label = "import from nowhere"',
    "// see ./y.ts",
    'importantThing("./z")',
  ])
    assert.deepEqual(importedPaths(text), [], text)
})
