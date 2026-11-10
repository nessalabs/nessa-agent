import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import test from "node:test"

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8")

/** The `members` array of the root `[workspace]` table. */
function workspaceMembers(manifest) {
  const block = manifest.match(/^\[workspace\][^[]*?members\s*=\s*\[([^\]]*)\]/ms)
  if (!block) return null
  return [...block[1].matchAll(/"([^"]+)"/g)].map((match) => match[1])
}

/** Whether a member's manifest takes its lints from the workspace table. */
function inheritsWorkspaceLints(manifest) {
  return /^\[lints\]\s*\n(?:\s*#[^\n]*\n)*\s*workspace\s*=\s*true\s*$/m.test(manifest)
}

test("every workspace crate inherits the root lints table", () => {
  const root = read("Cargo.toml")
  assert.match(root, /^\[workspace\.lints\.clippy\]$/m)
  const members = workspaceMembers(root)
  assert.ok(members && members.length > 0, "root Cargo.toml lists no members")
  const missing = members.filter(
    (member) => !inheritsWorkspaceLints(read(`${member}/Cargo.toml`)),
  )
  assert.deepEqual(
    missing,
    [],
    "add `[lints]\\nworkspace = true` to these Cargo.toml files",
  )
})

test("the member and opt-in readers see what cargo sees", () => {
  assert.deepEqual(
    workspaceMembers('[workspace]\nresolver = "2"\nmembers = [\n  "a", "b/c"]\n'),
    ["a", "b/c"],
  )
  assert.equal(inheritsWorkspaceLints("[lints]\nworkspace = true\n"), true)
  assert.equal(inheritsWorkspaceLints("[lints]\nworkspace = false\n"), false)
  assert.equal(inheritsWorkspaceLints('[lints.clippy]\ntodo = "warn"\n'), false)
  assert.equal(inheritsWorkspaceLints("# [lints]\n# workspace = true\n"), false)
})
