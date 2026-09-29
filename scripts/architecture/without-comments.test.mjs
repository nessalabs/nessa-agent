/** Bare Node, no `node_modules`, like the scanner it tests. */
import assert from "node:assert/strict"
import test from "node:test"

import { withoutComments } from "./without-comments.mjs"

test("blanks line, block and doc comments, keeping every other character where it was", () => {
  const text = 'a // import "../x"\n/* b\n * c */ d\n/** e */ f'
  const blanked = withoutComments(text)
  assert.equal(blanked.length, text.length)
  assert.equal(blanked.split("\n").length, text.split("\n").length)
  assert.equal(blanked.replace(/\s+/g, " ").trim(), "a d f")
})

test("keeps a `//` or `/*` inside a string, template or regular expression", () => {
  for (const code of [
    'const url = "https://example.com/x"',
    "const quote = /[\"'/]/g",
    'const t = `a ${"b"} // d ${x}`',
    "const n = a / b / c",
  ])
    assert.equal(withoutComments(code), code, code)
  assert.equal(
    withoutComments('const t = `${"b" /* c */}`'),
    'const t = `${"b"        }`',
  )
})
