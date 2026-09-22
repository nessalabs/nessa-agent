import { execFileSync } from "node:child_process"
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { join } from "node:path"

import { materializeBinLinks } from "./materialize-bin-links.mjs"
import { runtimeFingerprint } from "./runtime-fingerprint.mjs"

// Each harness and the package it exists to pin are named explicitly. Reading
// whichever dependency happens to come first would silently change the runtime
// identity when a harness gains another dependency.
const HARNESSES = {
  "claude-acp": "@agentclientprotocol/claude-agent-acp",
  "codex-acp": "@agentclientprotocol/codex-acp",
}

/** Read the native Rust target without guessing it from Node's platform names. */
export function rustHostTarget(versionOutput) {
  const host = versionOutput.match(/^host: (.+)$/m)?.[1]
  if (!host) throw new Error("rustc did not report its host target")
  return host
}

/**
 * Assemble the platform-independent part of a desktop runtime. The active
 * macOS adapter supplies Node and performs the signing its package requires.
 */
export function assembleDesktopRuntime({
  root,
  out,
  executables,
  requestedTarget,
  prepareNode,
  finalizeExecutables,
  run = execFileSync,
}) {
  const host = rustHostTarget(run("rustc", ["-vV"], { encoding: "utf8" }))
  if (requestedTarget && requestedTarget !== host)
    throw new Error("Build the desktop runtime on the target architecture")

  run("cargo", ["build", "--release", "-p", "nessa-server", "-p", "nessa-mcp"], {
    cwd: root,
    stdio: "inherit",
  })
  const metadata = JSON.parse(
    run("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
      cwd: root,
      encoding: "utf8",
    }),
  )
  if (typeof metadata.target_directory !== "string")
    throw new Error("cargo metadata did not report its target directory")
  for (const name of [executables.gateway, executables.mcp]) {
    if (!existsSync(join(metadata.target_directory, "release", name)))
      throw new Error(`cargo did not produce the desktop runtime executable: ${name}`)
  }

  // Rebuild the shipped tree so removed resources cannot survive another build.
  // Target and metadata validation happen first so a configuration error leaves
  // the last complete runtime intact.
  rmSync(out, { recursive: true, force: true })
  mkdirSync(out, { recursive: true })
  for (const name of [executables.gateway, executables.mcp])
    cpSync(join(metadata.target_directory, "release", name), join(out, name))

  const nodeVersion = prepareNode({ executable: executables.node, out })
  if (typeof nodeVersion !== "string" || nodeVersion.length === 0)
    throw new Error("Node preparation did not report its version")
  const harnesses = {}
  for (const [name, pinned] of Object.entries(HARNESSES)) {
    const harness = join(out, name)
    mkdirSync(harness, { recursive: true })
    for (const file of ["package.json", "package-lock.json"])
      cpSync(join(root, "crates/nessa-sdk/harnesses", name, file), join(harness, file))
    run("npm", ["ci", "--omit=dev", "--no-audit", "--no-fund"], {
      cwd: harness,
      stdio: "inherit",
    })
    materializeBinLinks(join(harness, "node_modules"))
    const manifest = JSON.parse(readFileSync(join(harness, "package.json"), "utf8"))
    const version = manifest.dependencies?.[pinned]
    if (!version) throw new Error(`${name} no longer pins ${pinned}`)
    harnesses[name] = version
  }
  cpSync(join(root, "crates/nessa-sdk/data/models.json"), join(out, "models.json"))

  finalizeExecutables({ executables, out })
  const fingerprint = runtimeFingerprint(out)
  const manifest = {
    node: nodeVersion,
    claudeAcp: harnesses["claude-acp"],
    codexAcp: harnesses["codex-acp"],
    target: host,
    fingerprint,
  }
  writeFileSync(join(out, "manifest.json"), JSON.stringify(manifest, null, 2))
  return manifest
}
