#!/usr/bin/env node
/**
 * Turns on the checked-in hooks in `.githooks/` for this checkout. `pnpm
 * install` runs it (`prepare`), so a fresh clone, a worktree and a cloud
 * session all get the pre-push hook (`scripts/pre-push.mjs`).
 *
 * It sets `core.hooksPath` only when nothing has set it. A person who points
 * git at hooks of their own keeps them, and is told how to add ours. Outside a
 * git checkout of its own (a tarball, even one unpacked inside another
 * repository) it does nothing.
 */
import { spawnSync } from "node:child_process"
import { realpathSync } from "node:fs"
import { fileURLToPath, pathToFileURL } from "node:url"

export const HOOKS = ".githooks"

/**
 * What to do, given `core.hooksPath` as git reports it (`null` when unset) and
 * whether this is a git checkout at all.
 */
export function hooksAction({ inCheckout, hooksPath }) {
  if (!inCheckout) return { set: false }
  if (hooksPath === null) return { set: true }
  if (hooksPath === HOOKS) return { set: false }
  return {
    set: false,
    note:
      `core.hooksPath is ${hooksPath}, so the checked-in pre-push hook is off here. ` +
      `Run \`git config core.hooksPath ${HOOKS}\` to use it, or call .githooks/pre-push from your own.`,
  }
}

function git(args) {
  const result = spawnSync("git", args, { encoding: "utf8" })
  if (result.error) return null
  return { status: result.status, stdout: result.stdout.trim() }
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) {
  // Windows paths differ in slashes and drive-letter case between git and Node.
  const same = (path) => {
    const real = realpathSync.native(path)
    return process.platform === "win32" ? real.toLowerCase() : real
  }
  const own = same(fileURLToPath(new URL("..", import.meta.url)))
  const top = git(["rev-parse", "--show-toplevel"])
  const configured = git(["config", "--get", "core.hooksPath"])
  const action = hooksAction({
    inCheckout: top?.status === 0 && same(top.stdout) === own,
    hooksPath: configured?.status === 0 ? configured.stdout : null,
  })
  if (action.note) process.stderr.write(`${action.note}\n`)
  if (action.set) {
    const result = git(["config", "--local", "core.hooksPath", HOOKS])
    if (result?.status !== 0)
      process.stderr.write("Could not turn on the checked-in git hooks.\n")
  }
}
