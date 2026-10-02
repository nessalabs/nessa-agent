/** Bare Node, no `node_modules`, like the rule it tests. */
import assert from "node:assert/strict"
import test from "node:test"

import { desktopVerticalViolations, desktopVerticals } from "./desktop-verticals.mjs"
import { importedPaths } from "./imported-paths.mjs"

const refused = (path, text) => desktopVerticalViolations(path, importedPaths(text))

/** A file two folders deep in each vertical, so `../../<vertical>` reaches the others. */
const fileIn = (vertical) => `src/desktop/${vertical}/ui/probe/file.tsx`

/**
 * Every form of import `imported-paths.mjs` reads, of `target`. Quoted by a
 * helper, so this file's own text names no import for the check to read.
 */
const quoted = (target) => `"${target}"`
const forms = (target) => [
  `import { x } from ${quoted(target)}`,
  `import type { X } from ${quoted(target)}`,
  `export { x } from ${quoted(target)}`,
  `import ${quoted(`${target}/ui/sheet.css`)}`,
  `const m = await import(${quoted(target)})`,
  `vi.mock(${quoted(target)}, () => ({}))`,
  `vi.doMock(${quoted(target)})`,
  `const real = await vi.importActual(${quoted(target)})`,
  `const mocked = await vi.importMock<typeof X>(${quoted(target)})`,
]

test("each vertical refuses every later one, in every form of import", () => {
  for (const [at, from] of desktopVerticals.entries())
    for (const to of desktopVerticals.slice(at + 1))
      for (const text of forms(`../../../${to}`))
        assert.equal(refused(fileIn(from), text).length, 1, `${from} → ${to}: ${text}`)
})

test("each vertical imports every earlier one", () => {
  for (const [at, from] of desktopVerticals.entries())
    for (const to of desktopVerticals.slice(0, at))
      for (const text of [
        `import { x } from ${quoted(`../../../${to}`)}`,
        `import type { X } from ${quoted(`../../../${to}/model/file`)}`,
      ])
        assert.deepEqual(refused(fileIn(from), text), [], `${from} → ${to}: ${text}`)
})

test("a vertical imports itself", () => {
  for (const vertical of desktopVerticals)
    assert.deepEqual(
      refused(fileIn(vertical), 'import { x } from "../../model/file"'),
      [],
      vertical,
    )
})

test("what lies outside the four verticals is not this rule's business", () => {
  // The window's composition imports every vertical.
  assert.deepEqual(
    refused("src/desktop/main.tsx", 'import { x } from "./experiments"'),
    [],
  )
  assert.deepEqual(
    refused("src/desktop/dependencies.ts", 'import { x } from "./workspace"'),
    [],
  )
  // A vertical importing a shared part or split panes, or a package.
  assert.deepEqual(
    refused(fileIn("widgets"), 'import { x } from "../../../split-panes"'),
    [],
  )
  assert.deepEqual(refused(fileIn("widgets"), 'import { x } from "../../../ui/menu"'), [])
  assert.deepEqual(refused(fileIn("widgets"), 'import { posix } from "node:path"'), [])
  // A folder whose name only begins like a vertical's.
  assert.deepEqual(
    refused(fileIn("widgets"), 'import { x } from "../../../workspaces-old"'),
    [],
  )
})

test("an import named in a comment is not an import", () => {
  assert.deepEqual(
    refused(fileIn("widgets"), '// import { x } from "../../../workspace"\nconst a = 1'),
    [],
  )
})
