#!/usr/bin/env node
/**
 * Prepare every source-linked package from the pinned Nessa UI workspace.
 *
 * `nessa-ui-revision` is the only pin. This script has three entry points:
 *
 *   node scripts/ensure-nessa-ui.mjs           preinstall: fill or advance .vendor
 *   node scripts/ensure-nessa-ui.mjs --check   compare .vendor with the pin, never fetch
 *   node scripts/ensure-nessa-ui.mjs --update  pnpm ui:types: the same as preinstall
 *
 * pnpm skips preinstall when the lockfile is already satisfied, so after a
 * pull that only moves the pin, `pnpm install` does nothing. `pnpm ui:types`
 * is the reliable way to advance; `--check` says when that is needed.
 */
import {
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
} from "node:fs"
import { spawnSync } from "node:child_process"
import { dirname, resolve } from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"

export const linkedWorkspacePackages = ["packages/react", "packages/agent-stream"]

function command(command, args, cwd) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" })
  if ((result.status ?? 1) !== 0)
    throw new Error(`${command} failed with ${result.status}`)
}

/** Run git and return its trimmed stdout, or null when it fails. */
function gitQuery(args, cwd) {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" })
  if (result.status !== 0) return null
  return result.stdout.trim()
}

export function ensureWorkspace(repoRoot, run = command) {
  const packages = linkedWorkspacePackages.map((directory) => {
    const manifest = resolve(repoRoot, directory, "package.json")
    if (!existsSync(manifest))
      throw new Error(`Missing linked workspace package ${directory}`)
    return JSON.parse(readFileSync(manifest, "utf8"))
  })
  const install = ["install", "--frozen-lockfile"]
  for (const pkg of packages) install.push("--filter", `${pkg.name}...`)
  run("pnpm", install, repoRoot)
  for (const pkg of packages) {
    if (pkg.scripts?.build) run("pnpm", ["--filter", pkg.name, "build"], repoRoot)
  }
}

export function readRevision(root) {
  const revision = readFileSync(resolve(root, "nessa-ui-revision"), "utf8").trim()
  if (!/^[a-f0-9]{40}$/.test(revision)) throw new Error("Invalid nessa-ui-revision")
  return revision
}

const hasPackages = (directory) =>
  linkedWorkspacePackages.every((pkg) =>
    existsSync(resolve(directory, pkg, "package.json")),
  )

/**
 * Compare the vendored checkout with the pin without touching the network.
 * Returns null when they agree, otherwise one message naming the remedy.
 */
export function checkVendor({ root, git = gitQuery } = {}) {
  const vendor = resolve(root, ".vendor/nessa_ui")
  const revision = readRevision(root)
  if (!hasPackages(vendor))
    return ".vendor/nessa_ui is missing or incomplete. Run `pnpm ui:types` to fetch it."
  const head = git(["rev-parse", "HEAD"], vendor)
  if (head === revision) return null
  return (
    `Vendored Nessa UI is at ${head?.slice(0, 7) ?? "an unknown commit"} but ` +
    `nessa-ui-revision wants ${revision.slice(0, 7)}.\n` +
    "Run `pnpm ui:types` to update it."
  )
}

export function prepareVendor({ root, run = command, git = gitQuery } = {}) {
  const vendor = resolve(root, ".vendor/nessa_ui")
  const repo = "https://github.com/nessalabs/nessa_ui.git"
  const revision = readRevision(root)
  const locals = [
    resolve(root, "../nessa/.claude/worktrees/imessage-composer-chat-ui"),
    resolve(root, "../nessa_ui"),
  ]
  const revisionAt = (directory) => git(["rev-parse", "HEAD"], directory) === revision
  const checkoutRevision = () => {
    run("git", ["fetch", "--depth", "1", "origin", revision], vendor)
    run("git", ["checkout", "--detach", revision], vendor)
  }

  mkdirSync(resolve(root, ".vendor"), { recursive: true })
  for (const local of locals) {
    if (!hasPackages(local) || !revisionAt(local)) continue
    if (existsSync(vendor)) rmSync(vendor, { recursive: true, force: true })
    symlinkSync(local, vendor)
    break
  }
  if (!hasPackages(vendor)) {
    if (existsSync(vendor)) rmSync(vendor, { recursive: true, force: true })
    run("git", ["clone", "--no-checkout", "--depth", "1", repo, vendor], root)
    checkoutRevision()
  }
  if (!revisionAt(vendor)) {
    // A symlinked sibling checkout belongs to someone else; never move it.
    if (lstatSync(vendor).isSymbolicLink())
      throw new Error(
        `Nessa UI must be exactly ${revision}, but .vendor/nessa_ui links to a ` +
          "checkout at another commit. Move that checkout, or remove the link " +
          "and run pnpm install to clone the pin.",
      )
    // A managed clone is disposable, so it is advanced on its own — unless it
    // carries edits, which are never thrown away.
    const dirty = git(["status", "--porcelain"], vendor)
    if (dirty === null || dirty)
      throw new Error(
        "Preserving changes in the Nessa UI clone; commit or discard them, " +
          "then run pnpm ui:types.",
      )
    checkoutRevision()
  }
  ensureWorkspace(vendor, run)
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
  if (process.argv.includes("--check")) {
    const problem = checkVendor({ root })
    if (problem) {
      console.error(problem)
      process.exit(1)
    }
  } else {
    // --update is the explicit spelling of what preinstall does anyway.
    prepareVendor({ root })
  }
}
