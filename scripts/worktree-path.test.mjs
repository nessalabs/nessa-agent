import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { readFileSync } from "node:fs"
import test from "node:test"

/**
 * One branch, one directory, both ways.
 *
 * `scripts/worktree.sh` used to turn a slash into a dash to make a branch name
 * usable as a directory, which is lossy: a dash is an ordinary character in a
 * name, so `a/b` and `a-b` both arrived at `...-a-b`. Nothing downstream could
 * tell the two apart, and Claude Code's WorktreeCreate hook handed an agent a
 * checkout sitting on the other branch to commit to.
 *
 * The fix is not a collision check, it is a mapping that cannot collide: a
 * slash becomes a dot, and `check_name` does not admit a dot. These tests hold
 * that property rather than the spelling of it — the only thing that must stay
 * true is that no two accepted names reach one path.
 */

const script = new URL("worktree.sh", import.meta.url).pathname
const root = new URL("..", import.meta.url).pathname

/** The path the script says a branch belongs at. */
const pathFor = (name) =>
  execFileSync("bash", [script, "path", "--derived", name], {
    cwd: root,
    encoding: "utf8",
  }).trim()

/**
 * Names that differ only in the characters the mapping has to touch, plus the
 * shapes a real branch takes here: Claude Code names branches `claude/<slug>`,
 * and people write `fix/thing` and `fix-thing`.
 */
const names = [
  "a/b",
  "a-b",
  "a_b",
  "a/b/c",
  "a-b-c",
  "a/b-c",
  "a-b/c",
  "claude/ci-performance-improvements-1fbc3c",
  "claude-ci-performance-improvements-1fbc3c",
  "fix/thing",
  "fix-thing",
  "release/1/2",
  "release-1-2",
]

test("no two names the script accepts reach one directory", () => {
  const seen = new Map()
  for (const name of names) {
    const path = pathFor(name)
    const clash = seen.get(path)
    assert.equal(
      clash,
      undefined,
      `'${name}' and '${clash}' both map to ${path}\n` +
        "  a worktree for one would be handed to the other to commit to.",
    )
    seen.set(path, name)
  }
  assert.equal(seen.size, names.length)
})

/** The specific pair that was reported, kept by name so it cannot come back. */
test("the reported collision stays fixed", () => {
  assert.notEqual(pathFor("sel/collide"), pathFor("sel-collide"))
})

/**
 * The mapping is only unambiguous because a dot cannot arrive any other way.
 * Admitting one into `check_name` would silently make it lossy again, and the
 * comment saying so is easy to delete, so this reads the rule itself.
 */
test("the name alphabet still excludes the character that stands for a slash", () => {
  const source = readFileSync(script, "utf8")
  const alphabet = source.match(/\[\[ "\$1" =~ \^\[([^\]]+)\]\+\$ \]\]/)
  assert.ok(alphabet, "could not find the check_name pattern to read")
  assert.ok(
    !alphabet[1].includes("."),
    "check_name now accepts a dot, so a dot no longer only means a slash:\n" +
      "  pick a separator outside the alphabet, or worktree_path is lossy again.",
  )
})

test("a name the alphabet rejects is refused rather than placed somewhere odd", () => {
  assert.throws(() => pathFor("has space"), /invalid name/)
  assert.throws(() => pathFor("has.dot"), /invalid name/)
})
