#!/usr/bin/env node
/**
 * Puts `@nessa-ui/react` on disk so `link:.vendor/nessa_ui/packages/react`
 * resolves. The chat kit is not on npm; a git worktree of nessa_ui used to
 * sit at a machine-specific path, and `pnpm install` failed anywhere else.
 *
 * Prefer a sibling checkout if one is already there (the original worktree,
 * or a clone named `nessa_ui`). Otherwise clone nessalabs/nessa_ui at the reviewed revision.
 */
import {
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  lstatSync,
} from "node:fs"
import { spawnSync } from "node:child_process"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const vendor = resolve(root, ".vendor/nessa_ui")
const repo = "https://github.com/nessalabs/nessa_ui.git"
const revision = readFileSync(resolve(root, "nessa-ui-revision"), "utf8").trim()
if (!/^[a-f0-9]{40}$/.test(revision)) throw new Error("Invalid nessa-ui-revision")
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
  // Source consumption needs the UI package's dependencies installed in its
  // own workspace. Always reconcile the install after checkout changes.
  const { name } = JSON.parse(
    readFileSync(resolve(repoRoot, "packages/react/package.json"), "utf8"),
  )
  run("pnpm", ["install", "--frozen-lockfile", "--filter", `${name}...`], repoRoot)
}

function includesRevision(dir) {
  return (
    spawnSync("git", ["merge-base", "--is-ancestor", revision, "HEAD"], { cwd: dir })
      .status === 0
  )
}

function checkoutRevision() {
  run("git", ["fetch", "--depth", "1", "origin", revision], vendor)
  run("git", ["checkout", "--detach", revision], vendor)
}

function placeVendor() {
  mkdirSync(resolve(root, ".vendor"), { recursive: true })
  for (const local of locals) {
    if (!hasReact(local) || !includesRevision(local)) continue
    if (existsSync(vendor)) rmSync(vendor, { recursive: true, force: true })
    symlinkSync(local, vendor)
    return
  }
  if (hasReact(vendor)) return
  if (existsSync(vendor)) rmSync(vendor, { recursive: true, force: true })
  run("git", ["clone", "--no-checkout", "--depth", "1", repo, vendor])
  checkoutRevision()
}

placeVendor()
if (!includesRevision(vendor)) {
  if (!update || lstatSync(vendor).isSymbolicLink()) {
    throw new Error(
      `Nessa UI must include ${revision}. Update the linked checkout, or run pnpm ui:types for a managed clone.`,
    )
  }
  const dirty = spawnSync("git", ["status", "--porcelain"], {
    cwd: vendor,
    encoding: "utf8",
  })
  if (dirty.status !== 0 || dirty.stdout.trim())
    throw new Error("Preserving changes in the Nessa UI clone; update it manually.")
  checkoutRevision()
}
ensureWorkspace(vendor)
