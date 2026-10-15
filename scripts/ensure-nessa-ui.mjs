#!/usr/bin/env node
/**
 * Prepare every source-linked package from the pinned Nessa UI workspace.
 *
 * `nessa-ui-revision` is the only pin. This script has three entry points:
 *
 *   node scripts/ensure-nessa-ui.mjs           preinstall: fill or advance .vendor
 *   node scripts/ensure-nessa-ui.mjs --check   check the pin and declared entry files, never fetch
 *   node scripts/ensure-nessa-ui.mjs --update  pnpm ui:types: the same as preinstall
 *
 * pnpm skips preinstall when the lockfile is already satisfied, so after a
 * pull that only moves the pin, `pnpm install` does nothing. `pnpm ui:types`
 * is the reliable way to advance; `--check` says when that is needed.
 */
import { existsSync, lstatSync, mkdirSync, readFileSync, rmSync, statSync } from "node:fs"
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
  // Workspace installs and builds write beneath these roots. A root-level
  // node_modules symlink can borrow another checkout's dependencies, so unlink
  // only those roots; package links inside an owned node_modules stay intact.
  for (const directory of [".", ...linkedWorkspacePackages]) {
    const nodeModules = resolve(repoRoot, directory, "node_modules")
    if (lstatSync(nodeModules, { throwIfNoEntry: false })?.isSymbolicLink())
      rmSync(nodeModules)
  }
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

function declaredFiles(value) {
  // Pattern declarations describe a family of subpaths, not a concrete file.
  if (typeof value === "string") return value.includes("*") ? [] : [value]
  if (value && typeof value === "object")
    return Object.values(value).flatMap(declaredFiles)
  return []
}

// Substitution arrays are ordered fallbacks. If a pattern participates, leave
// that mapping to TypeScript; only fully concrete groups can be checked here.
function declaredTypeAlternatives(value) {
  if (Array.isArray(value)) {
    const files = declaredFiles(value)
    return files.length > 0 && files.length === value.length ? [files] : []
  }
  if (value && typeof value === "object")
    return Object.values(value).flatMap(declaredTypeAlternatives)
  return []
}

/**
 * Compare the vendored checkout with the pin without touching the network.
 * Checks concrete declared package entry files too. Returns null when ready, otherwise
 * one message naming the preparation remedy.
 */
export function checkVendor({ root, git = gitQuery } = {}) {
  const vendor = resolve(root, ".vendor/nessa_ui")
  const revision = readRevision(root)
  if (!hasPackages(vendor))
    return ".vendor/nessa_ui is missing or incomplete. Run `pnpm ui:types` to fetch it."
  const head = git(["rev-parse", "HEAD"], vendor)
  if (head !== revision)
    return (
      `Vendored Nessa UI is at ${head?.slice(0, 7) ?? "an unknown commit"} but ` +
      `nessa-ui-revision wants ${revision.slice(0, 7)}.\n` +
      "Run `pnpm ui:types` to update it."
    )
  for (const directory of linkedWorkspacePackages) {
    const packageRoot = resolve(vendor, directory)
    const manifest = JSON.parse(
      readFileSync(resolve(packageRoot, "package.json"), "utf8"),
    )
    // The package owns its entry points; consumers must not copy a dist list.
    const entries = [
      ...new Set([
        ...declaredFiles(manifest.exports),
        ...declaredFiles(manifest.main),
        ...declaredFiles(manifest.module),
        ...declaredFiles(manifest.types),
      ]),
    ].map((entry) => [entry])
    for (const alternatives of [
      ...entries,
      ...declaredTypeAlternatives(manifest.typesVersions),
    ]) {
      const ready = alternatives.some((entry) => {
        const file = statSync(resolve(packageRoot, entry), { throwIfNoEntry: false })
        if (!file?.isFile() || file.size === 0) return false
        return true
      })
      if (!ready)
        return `Vendored Nessa UI entry ${directory}/${alternatives.join(" or ")} is missing or empty. Run \`pnpm ui:types\` to rebuild it.`
    }
  }
  return null
}

export function prepareVendor({ root, run = command, git = gitQuery } = {}) {
  const vendor = resolve(root, ".vendor/nessa_ui")
  const repo = "https://github.com/nessalabs/nessa_ui.git"
  const revision = readRevision(root)
  const revisionAt = (directory) => git(["rev-parse", "HEAD"], directory) === revision
  const checkoutRevision = () => {
    run("git", ["fetch", "--depth", "1", "origin", revision], vendor)
    run("git", ["checkout", "--detach", revision], vendor)
  }

  const vendorRoot = resolve(root, ".vendor")
  if (lstatSync(vendorRoot, { throwIfNoEntry: false })?.isSymbolicLink())
    throw new Error(
      "Preserving linked .vendor directory; replace it with a directory owned " +
        "by this checkout, then run pnpm ui:types.",
    )
  mkdirSync(vendorRoot, { recursive: true })
  // Build artifacts belong to this checkout. Remove only a borrowed link,
  // including a dangling one, before cloning; its target remains untouched.
  if (lstatSync(vendor, { throwIfNoEntry: false })?.isSymbolicLink()) rmSync(vendor)
  if (!hasPackages(vendor)) {
    if (existsSync(vendor)) rmSync(vendor, { recursive: true, force: true })
    run("git", ["clone", "--no-checkout", "--depth", "1", repo, vendor], root)
    checkoutRevision()
  }
  if (!revisionAt(vendor)) {
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
