import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import test from "node:test"
import { standDownPlacementViolations } from "./stand-down-placement.mjs"

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..")
const PATH = "scripts/dev-agent-config.mjs"

/** A file shaped like the real one: helpers, then the guarded entry point. */
function script({ body = "", entry = "  process.exit(1)\n" } = {}) {
  // The real file resolves its own directory from `import.meta.url` near the
  // top, long before the guard. A rule that anchored on the first mention
  // would treat everything after this line as the entry point and pass on
  // anything, so every fixture here carries it.
  const preamble = `const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")\n`
  return `${preamble}function skip(reason) {\n  throw new StoodDown(reason)\n}\n${body}if (invoked === realpathSync(fileURLToPath(import.meta.url))) {\n${entry}}\n`
}

test("the script in this repository satisfies the rule", () => {
  const source = readFileSync(join(root, PATH), "utf8")
  assert.deepEqual(standDownPlacementViolations(PATH, source), [])
})

test("a process.exit below the entry point is a violation", () => {
  const violations = standDownPlacementViolations(
    PATH,
    script({ body: "function skip(reason) {\n  process.exit(0)\n}\n" }),
  )
  assert.equal(violations.length, 1)
  assert.match(violations[0], /only in this script's entry point/)
})

test("the exit the entry point itself makes is allowed", () => {
  assert.deepEqual(standDownPlacementViolations(PATH, script()), [])
})

test("an entry point that is not behind the guard is a violation", () => {
  const violations = standDownPlacementViolations(
    PATH,
    `const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")\nmain()\nprocess.exit(1)\n`,
  )
  // Both halves fail: there is no guard, and the exit is therefore in the body.
  assert.equal(violations.length, 2)
  assert.match(violations[0], /import\.meta\.url guard/)
})

test("process.exit is matched on a word boundary", () => {
  // `process.exitCode` is a different thing: it sets a status without ending
  // the caller's process, which is exactly what this rule wants.
  assert.deepEqual(
    standDownPlacementViolations(
      PATH,
      script({ body: "function skip() {\n  process.exitCode = 1\n}\n" }),
    ),
    [],
  )
})

test("the rule reads only the script it is about", () => {
  assert.deepEqual(
    standDownPlacementViolations("scripts/check-protocol.mjs", "process.exit(0)"),
    [],
  )
})

test("the script's own use of import.meta.url does not open the entry point", () => {
  // The regression this rule was first written with: anchoring on the first
  // `import.meta.url` rather than on the invocation guard left only the file's
  // opening lines under scrutiny, and a `process.exit` in `skip` passed.
  const violations = standDownPlacementViolations(
    PATH,
    script({ body: "function skip() {\n  process.exit(0)\n}\n" }),
  )
  assert.equal(violations.length, 1)
  assert.match(violations[0], /only in this script's entry point/)
})
