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
    'const i = await vi.importActual<typeof import("./i")>("./i")',
    'const j = await vi.importMock("./j")',
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
    "./i",
    "./i",
    "./j",
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

test("an import written in a comment is documentation, not an import", () => {
  const text = [
    '// import "../../../workspace"',
    '/* import x from "../../../workspace/model"',
    ' * import "../../no"',
    " */",
    '/** vi.mock("../../mocked") */',
    'import a from "./a" // import "../trailing"',
  ].join("\n")
  assert.deepEqual(importedPaths(text), ["./a"])
})

test("a `//` or `/*` inside a string, template or regular expression is not a comment", () => {
  const text = [
    'const url = "https://example.com/x"; import "./after-string.css"',
    'const quote = /["\'/]/g; import "./after-regex.css"',
    'const t = `a ${"b" /* c */} // d ${x}`; import "./after-template.css"',
    'const n = a / b; import("./after-division")',
  ].join("\n")
  assert.deepEqual(importedPaths(text), [
    "./after-string.css",
    "./after-regex.css",
    "./after-template.css",
    "./after-division",
  ])
})
