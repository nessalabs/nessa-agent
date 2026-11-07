import { execFileSync } from "node:child_process"
import {
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { dirname, join } from "node:path"

import { hostBuildName, hostBuildPin } from "./host-builds.mjs"
import { materializeBinLinks } from "./materialize-bin-links.mjs"
import { runtimeFingerprint, verifyRuntimeFingerprint } from "./runtime-fingerprint.mjs"

// Each harness and the package it exists to pin are named explicitly. Reading
// whichever dependency happens to come first would silently change the runtime
// identity when a harness gains another dependency.
const HARNESSES = {
  "claude-acp": "@agentclientprotocol/claude-agent-acp",
  "codex-acp": "@agentclientprotocol/codex-acp",
}

function validatePreparedRuntime(out, executables) {
  for (const name of [...Object.values(executables), "models.json", "NODE-LICENSE"]) {
    const path = join(out, name)
    const stat = lstatSync(path)
    if (!stat.isFile() || stat.size === 0)
      throw new Error(`Desktop runtime output is not a nonempty file: ${name}`)
  }
  if (process.platform !== "win32")
    for (const name of Object.values(executables)) {
      if ((lstatSync(join(out, name)).mode & 0o111) === 0)
        throw new Error(`Desktop runtime output is not executable: ${name}`)
    }
  for (const name of Object.keys(HARNESSES)) {
    for (const file of ["package.json", "package-lock.json"])
      if (!lstatSync(join(out, name, file)).isFile())
        throw new Error(`Desktop runtime harness is missing ${name}/${file}`)
    const bin = join(out, name, "node_modules/.bin")
    const launchers = readdirSync(bin)
    if (launchers.length === 0)
      throw new Error(`Desktop runtime harness has no launchers: ${name}`)
    for (const launcher of launchers) {
      if (!lstatSync(join(bin, launcher)).isFile())
        throw new Error(`Desktop runtime harness launcher was not materialized: ${name}`)
    }
  }
}

/**
 * The Linux a Boat VM runs, and the oldest glibc its `nessa` is linked
 * against (the release builds it on Ubuntu 22.04, glibc 2.35).
 */
export const BOAT_HOST_TARGET = "x86_64-unknown-linux-gnu"
const BOAT_HOST_GLIBC = "2.35"

/**
 * The `nessa` a local build carries for Boat VMs, and the build environment
 * that pins it (#734's pins, read from the app rather than a release).
 *
 * A gateway installs itself on a host with no copy of its build; on another
 * platform it sends that platform's build, which it knows only by the pin it
 * was compiled with. A release compiles those pins in (NESSA_HOST_BUILDS);
 * a local Mac build had none, so it could never start a Boat chat. So a local
 * build cross-builds the Linux `nessa` from the same sources first, compiles
 * the gateway with its pin and NESSA_HOST_BUILDS_FROM=bundled, and ships the
 * file in runtime/host-builds/, where the gateway reads it.
 *
 * Returns undefined when the environment already pins release builds (the
 * release workflow) or opts out with NESSA_SKIP_HOST_BUILDS=1.
 */
export function crossBuildHostBuilds({ root, environment, run }) {
  if (environment.NESSA_HOST_BUILDS || environment.NESSA_HOST_BUILDS_FROM)
    return undefined
  if (environment.NESSA_SKIP_HOST_BUILDS === "1") {
    process.stderr.write(
      "→ NESSA_SKIP_HOST_BUILDS=1: this app cannot start chats in Boat VMs\n",
    )
    return undefined
  }
  try {
    run("cargo", ["zigbuild", "--help"], { cwd: root, stdio: "ignore" })
  } catch {
    throw new Error(
      "A local build carries the Linux `nessa` Boat VMs run, cross-built with cargo-zigbuild. " +
        `Install it (brew install zig && cargo install cargo-zigbuild && rustup target add ${BOAT_HOST_TARGET}), ` +
        "or set NESSA_SKIP_HOST_BUILDS=1 to build an app that cannot start Boat chats.",
    )
  }
  run(
    "cargo",
    [
      "zigbuild",
      "--release",
      "-p",
      "nessa-server",
      "--bin",
      "nessa",
      "--target",
      `${BOAT_HOST_TARGET}.${BOAT_HOST_GLIBC}`,
    ],
    { cwd: root, stdio: "inherit" },
  )
  const metadata = JSON.parse(
    run("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
      cwd: root,
      encoding: "utf8",
    }),
  )
  const binary = join(metadata.target_directory, BOAT_HOST_TARGET, "release", "nessa")
  if (!existsSync(binary))
    throw new Error(`cargo zigbuild did not produce ${BOAT_HOST_TARGET}'s nessa`)
  const pin = hostBuildPin(BOAT_HOST_TARGET, readFileSync(binary))
  process.stdout.write(`→ Boat VMs get ${hostBuildName(BOAT_HOST_TARGET)} (${pin})\n`)
  return {
    files: { [hostBuildName(BOAT_HOST_TARGET)]: binary },
    environment: { NESSA_HOST_BUILDS: pin, NESSA_HOST_BUILDS_FROM: "bundled" },
  }
}

/** Read the native Rust target without guessing it from Node's platform names. */
export function rustHostTarget(versionOutput) {
  const host = versionOutput.match(/^host: (.+)$/m)?.[1]
  if (!host) throw new Error("rustc did not report its host target")
  return host
}

/**
 * Assemble the platform-independent part of a desktop runtime. The active
 * platform adapter supplies Node and performs the final checks its package requires.
 */
export function assembleDesktopRuntime({
  root,
  out,
  executables,
  expectedHostTarget,
  requestedTarget,
  prepareNode,
  finalizeExecutables,
  hostBuilds = false,
  environment = process.env,
  run = execFileSync,
}) {
  const host = rustHostTarget(run("rustc", ["-vV"], { encoding: "utf8" }))
  if (expectedHostTarget && host !== expectedHostTarget)
    throw new Error(
      `Desktop runtime requires the native Rust target ${expectedHostTarget}`,
    )
  if (requestedTarget && requestedTarget !== host)
    throw new Error("Build the desktop runtime on the target architecture")
  if (existsSync(out) && !lstatSync(out).isDirectory())
    throw new Error("Desktop runtime output must be an owned directory")

  // From this point onward a failure belongs to this preparation attempt. An
  // earlier manifest must not make its old tree look like the result.
  rmSync(join(out, "manifest.json"), { force: true })

  const carried = hostBuilds
    ? crossBuildHostBuilds({ root, environment, run })
    : undefined
  run("cargo", ["build", "--release", "-p", "nessa-server", "-p", "nessa-mcp"], {
    cwd: root,
    stdio: "inherit",
    ...(carried && { env: { ...environment, ...carried.environment } }),
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

  mkdirSync(dirname(out), { recursive: true })
  const stage = mkdtempSync(join(dirname(out), ".runtime-prepare-"))
  try {
    for (const name of [executables.gateway, executables.mcp])
      cpSync(join(metadata.target_directory, "release", name), join(stage, name))
    // Beside the gateway, where NESSA_HOST_BUILDS_FROM=bundled reads them.
    if (carried) {
      mkdirSync(join(stage, "host-builds"))
      for (const [name, binary] of Object.entries(carried.files))
        cpSync(binary, join(stage, "host-builds", name))
    }

    const nodeVersion = prepareNode({ executable: executables.node, out: stage })
    if (typeof nodeVersion !== "string" || nodeVersion.length === 0)
      throw new Error("Node preparation did not report its version")
    const harnesses = {}
    for (const [name, pinned] of Object.entries(HARNESSES)) {
      const harness = join(stage, name)
      mkdirSync(harness, { recursive: true })
      for (const file of ["package.json", "package-lock.json"])
        cpSync(join(root, "crates/nessa-sdk/harnesses", name, file), join(harness, file))
      run(
        "npm",
        [
          "ci",
          "--omit=dev",
          "--omit=optional",
          "--ignore-scripts",
          "--no-audit",
          "--no-fund",
        ],
        {
          cwd: harness,
          stdio: "inherit",
        },
      )
      materializeBinLinks(join(harness, "node_modules"))
      const harnessManifest = JSON.parse(
        readFileSync(join(harness, "package.json"), "utf8"),
      )
      const version = harnessManifest.dependencies?.[pinned]
      if (!version) throw new Error(`${name} no longer pins ${pinned}`)
      harnesses[name] = version
    }
    cpSync(join(root, "crates/nessa-sdk/data/models.json"), join(stage, "models.json"))

    finalizeExecutables({ executables, out: stage })
    validatePreparedRuntime(stage, executables)
    const fingerprint = runtimeFingerprint(stage)
    const manifest = {
      node: nodeVersion,
      claudeAcp: harnesses["claude-acp"],
      codexAcp: harnesses["codex-acp"],
      target: host,
      fingerprint,
    }
    writeFileSync(join(stage, "manifest.json"), JSON.stringify(manifest, null, 2))
    rmSync(out, { recursive: true, force: true })
    renameSync(stage, out)
    verifyRuntimeFingerprint(out)
    return manifest
  } finally {
    rmSync(stage, { recursive: true, force: true })
  }
}
