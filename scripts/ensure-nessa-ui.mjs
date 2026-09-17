#!/usr/bin/env node
/** Prepare every source-linked package from the pinned Nessa UI workspace. */
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

export function prepareVendor({ root, update = false, run = command } = {}) {
  const vendor = resolve(root, ".vendor/nessa_ui")
  const repo = "https://github.com/nessalabs/nessa_ui.git"
  const revision = readFileSync(resolve(root, "nessa-ui-revision"), "utf8").trim()
  if (!/^[a-f0-9]{40}$/.test(revision)) throw new Error("Invalid nessa-ui-revision")
  const locals = [
    resolve(root, "../nessa/.claude/worktrees/imessage-composer-chat-ui"),
    resolve(root, "../nessa_ui"),
  ]
  const hasPackages = (directory) =>
    linkedWorkspacePackages.every((pkg) =>
      existsSync(resolve(directory, pkg, "package.json")),
    )
  const revisionAt = (directory) =>
    spawnSync("git", ["rev-parse", "HEAD"], {
      cwd: directory,
      encoding: "utf8",
    }).stdout?.trim() === revision
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
    if (!update || lstatSync(vendor).isSymbolicLink())
      throw new Error(
        `Nessa UI must be exactly ${revision}; run pnpm ui:types to update it.`,
      )
    const dirty = spawnSync("git", ["status", "--porcelain"], {
      cwd: vendor,
      encoding: "utf8",
    })
    if (dirty.status !== 0 || dirty.stdout.trim())
      throw new Error("Preserving changes in the Nessa UI clone; update it manually.")
    checkoutRevision()
  }
  ensureWorkspace(vendor, run)
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) {
  const root = resolve(dirname(fileURLToPath(import.meta.url)), "..")
  prepareVendor({ root, update: process.argv.includes("--update") })
}
