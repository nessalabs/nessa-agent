// Build a relocatable, self-contained runtime. No user config or credentials enter the bundle.
import { execFileSync, spawnSync } from "node:child_process"
import { resolve, join } from "node:path"
import { pathToFileURL } from "node:url"
import { prepareBundledNode } from "./prepare-node.mjs"
import { assembleDesktopRuntime } from "./prepare-runtime.mjs"
import { runtimeExecutables } from "./runtime-layout.mjs"
import {
  runtimeEntitlements,
  signingArguments,
  signingIdentity,
  signingProblems,
} from "./runtime-signing.mjs"
/** Prepare the signed macOS runtime with the shared verified Node acquisition. */
export function prepareMacosRuntime({
  arch = process.arch,
  platform = process.platform,
  requestedTarget = process.env.TAURI_ENV_TARGET_TRIPLE,
  root = resolve(import.meta.dirname, "../.."),
} = {}) {
  if (platform !== "darwin") throw new Error("Bundled gateway packaging requires macOS")
  const out = join(root, "src-tauri/runtime")
  const cache = join(root, "target/desktop-downloads")
  const identity =
    signingIdentity(process.env.NESSA_RUNTIME_SIGNING_IDENTITY) ??
    signingIdentity(process.env.APPLE_SIGNING_IDENTITY)
  const executables = runtimeExecutables(platform)

  return assembleDesktopRuntime({
    root,
    out,
    executables,
    requestedTarget,
    prepareNode({ executable, out: runtime }) {
      return prepareBundledNode({
        arch,
        cache,
        executable,
        out: runtime,
        platform,
      })
    },
    finalizeExecutables({ executables, out: runtime }) {
      // Sign the nested executables before fingerprinting: Tauri treats resources
      // as files, so these ship with the signatures they have at this point.
      for (const name of Object.values(executables)) {
        const plist = runtimeEntitlements(name)
        execFileSync(
          "codesign",
          signingArguments(join(runtime, name), {
            identity,
            entitlements: plist ? join(root, "src-tauri", plist) : undefined,
          }),
          { stdio: "inherit" },
        )
      }
      // Read back before the bundler reaches Apple's notary service.
      if (identity) {
        const problems = Object.values(executables).flatMap((name) => {
          const shown = spawnSync(
            "codesign",
            ["--display", "--verbose=2", join(runtime, name)],
            { encoding: "utf8" },
          )
          if (shown.status !== 0)
            throw new Error(
              `Could not read the signature of runtime/${name}:\n${shown.stderr}`,
            )
          // codesign writes the display to stderr and nothing to stdout.
          return signingProblems(name, shown.stderr)
        })
        if (problems.length > 0)
          throw new Error(
            `The runtime was signed, but not the way Apple requires:\n- ${problems.join("\n- ")}`,
          )
      }
      process.stdout.write(
        identity
          ? `→ runtime executables signed with ${identity}, hardened and timestamped\n`
          : "→ runtime executables signed ad-hoc; this bundle cannot be notarized\n",
      )
    },
  })
}

const invoked = process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href
if (invoked) prepareMacosRuntime()
