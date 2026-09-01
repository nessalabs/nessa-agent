#!/usr/bin/env node
/**
 * Puts `@nessa-ui/react` on disk so `link:.vendor/nessa_ui/packages/react`
 * resolves. The chat kit is not on npm; a git worktree of nessa_ui used to
 * sit at a machine-specific path, and `pnpm install` failed anywhere else.
 *
 * Prefer a sibling checkout if one is already there (the original worktree,
 * or a clone named `nessa_ui`). Otherwise clone nessalabs/nessa_ui.
 */
import { existsSync, mkdirSync, rmSync, symlinkSync } from "node:fs"
import { spawnSync } from "node:child_process"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const vendor = resolve(root, ".vendor/nessa_ui")
const repo = "https://github.com/nessalabs/nessa_ui.git"
const update = process.argv.includes("--update")

const locals = [
  resolve(root, "../nessa/.claude/worktrees/imessage-composer-chat-ui"),
  resolve(root, "../nessa_ui"),
]

function hasReact(dir) {
  return existsSync(resolve(dir, "packages/react/package.json"))
}

function run(command, args, cwd) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" })
  if ((result.status ?? 1) !== 0) {
    process.exit(result.status ?? 1)
  }
}

function ensureWorkspace(repoRoot) {
  // The app compiles the kit from source, so the kit's own node_modules have
  // to exist. `workspace:*` (`@nessa-ui/agent-stream`) only resolves inside
  // this checkout.
  if (existsSync(resolve(repoRoot, "packages/react/node_modules/clsx"))) {
    return
  }
  run("pnpm", ["install", "--filter", "@nessa-ui/react..."], repoRoot)
}

function placeVendor() {
  mkdirSync(resolve(root, ".vendor"), { recursive: true })
  for (const local of locals) {
    if (!hasReact(local)) continue
    if (existsSync(vendor)) rmSync(vendor, { recursive: true, force: true })
    symlinkSync(local, vendor)
    return
  }
  if (hasReact(vendor)) return
  if (existsSync(vendor)) rmSync(vendor, { recursive: true, force: true })
  run("git", ["clone", "--depth", "1", repo, vendor])
}

placeVendor()
if (update) {
  run("git", ["-C", vendor, "pull", "--ff-only"])
}
ensureWorkspace(vendor)
