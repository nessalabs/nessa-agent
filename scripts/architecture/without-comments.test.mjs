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

test("reads a regular expression after a comment or a condition as one, so a `/*` in it hides nothing", () => {
  for (const code of [
    'const re = /* explanation */ /[/*]/; import("../../../workspace")',
    'if (ready) /[/*]/.test(x); import("../../../workspace")',
    'while (x) /\\/*/.exec(y); import("../../../workspace")',
  ]) {
    const blanked = withoutComments(code)
    assert.match(blanked, /import\("\.\.\/\.\.\/\.\.\/workspace"\)/, code)
  }
  // A `)` that ends a value: the `/` after it divides, and a comment after that is one.
  assert.equal(
    withoutComments("const n = (a + b) / c /* half */").trimEnd(),
    "const n = (a + b) / c",
  )
})
