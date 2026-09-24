// Build the inert Linux runtime resource. GatewayHost and release publication remain disabled.
import { execFileSync, spawnSync } from "node:child_process"
import { resolve, join } from "node:path"
import { pathToFileURL } from "node:url"

import { NODE_VERSION, prepareBundledNode } from "./prepare-node.mjs"
import { assembleDesktopRuntime } from "./prepare-runtime.mjs"
import { runtimeExecutables } from "./runtime-layout.mjs"

export const LINUX_RUNTIME_TARGET = "x86_64-unknown-linux-gnu"
const PROBE_TIMEOUT_MS = 5_000

function completedProbe(result, name) {
  if (result.error)
    throw new Error(`Could not execute runtime/${name}`, { cause: result.error })
  if (result.signal || typeof result.status !== "number")
    throw new Error(`runtime/${name} did not finish its bounded probe`)
  return result
}

/** Verify the three native entry points without starting a service. */
export function verifyLinuxRuntimeExecutables({ executables, out, probe = spawnSync }) {
  const options = { encoding: "utf8", timeout: PROBE_TIMEOUT_MS }
  const node = completedProbe(
    probe(join(out, executables.node), ["--version"], options),
    executables.node,
  )
  if (node.status !== 0 || node.stdout.trim() !== `v${NODE_VERSION}`)
    throw new Error(`runtime/${executables.node} did not report v${NODE_VERSION}`)

  const gateway = completedProbe(
    probe(join(out, executables.gateway), ["--help"], options),
    executables.gateway,
  )
  if (gateway.status !== 0 || !gateway.stdout.startsWith("Nessa\n"))
    throw new Error(`runtime/${executables.gateway} did not answer its help probe`)

  const mcp = completedProbe(
    probe(join(out, executables.mcp), ["--help"], options),
    executables.mcp,
  )
  if (mcp.status === 0 || !mcp.stderr.includes("expected --workspace PATH"))
    throw new Error(`runtime/${executables.mcp} did not answer its argument probe`)
}

/** Assemble the Linux x86_64 preparation foundation without enabling GatewayHost. */
export function prepareLinuxRuntime({
  arch = process.arch,
  platform = process.platform,
  requestedTarget = process.env.TAURI_ENV_TARGET_TRIPLE,
  root = resolve(import.meta.dirname, "../.."),
  run = execFileSync,
  probe = spawnSync,
  prepareNode = prepareBundledNode,
} = {}) {
  if (platform !== "linux")
    throw new Error("Bundled Linux runtime preparation requires Linux")
  if (arch !== "x64") throw new Error("Bundled Linux runtime preparation requires x64")
  if (requestedTarget && requestedTarget !== LINUX_RUNTIME_TARGET)
    throw new Error(`Bundled Linux runtime preparation requires ${LINUX_RUNTIME_TARGET}`)

  const out = join(root, "src-tauri/runtime")
  const executables = runtimeExecutables(platform)
  return assembleDesktopRuntime({
    root,
    out,
    executables,
    expectedHostTarget: LINUX_RUNTIME_TARGET,
    requestedTarget,
    run,
    prepareNode({ executable, out: runtime }) {
      return prepareNode({
        arch,
        cache: join(root, "target/desktop-downloads"),
        executable,
        out: runtime,
        platform,
      })
    },
    finalizeExecutables({ executables: names, out: runtime }) {
      verifyLinuxRuntimeExecutables({ executables: names, out: runtime, probe })
    },
  })
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) prepareLinuxRuntime()
