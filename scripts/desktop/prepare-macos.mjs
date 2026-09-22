// Build a relocatable, self-contained runtime. No user config or credentials enter the bundle.
import { execFileSync, spawnSync } from "node:child_process"
import { mkdirSync, cpSync, readFileSync, existsSync } from "node:fs"
import { resolve, join } from "node:path"
import { createHash } from "node:crypto"
import { assembleDesktopRuntime } from "./prepare-runtime.mjs"
import { runtimeExecutables } from "./runtime-layout.mjs"
import {
  runtimeEntitlements,
  signingArguments,
  signingIdentity,
  signingProblems,
} from "./runtime-signing.mjs"
const root = resolve(import.meta.dirname, "../..")
if (process.platform !== "darwin")
  throw new Error("Bundled gateway packaging currently supports macOS")
const target = process.env.TAURI_ENV_TARGET_TRIPLE
const out = join(root, "src-tauri/runtime")
const version = "26.8.1"
const archive = `node-v${version}-darwin-${process.arch}.tar.gz`
const cache = join(root, "target/desktop-downloads")
const identity =
  signingIdentity(process.env.NESSA_RUNTIME_SIGNING_IDENTITY) ??
  signingIdentity(process.env.APPLE_SIGNING_IDENTITY)
const executables = runtimeExecutables(process.platform)

assembleDesktopRuntime({
  root,
  out,
  executables,
  requestedTarget: target,
  prepareNode({ executable, out: runtime }) {
    mkdirSync(cache, { recursive: true })
    const file = join(cache, archive)
    const base = `https://nodejs.org/dist/v${version}/`
    const sums = execFileSync(
      "curl",
      ["--fail", "--silent", "--show-error", "--location", `${base}SHASUMS256.txt`],
      { encoding: "utf8" },
    )
    const expected = sums
      .split("\n")
      .find((line) => line.endsWith(`  ${archive}`))
      ?.split(" ")[0]
    if (!expected) throw new Error("Node archive is absent from release checksums")
    if (!existsSync(file))
      execFileSync("curl", ["--fail", "--location", "--output", file, base + archive], {
        stdio: "inherit",
      })
    if (createHash("sha256").update(readFileSync(file)).digest("hex") !== expected)
      throw new Error("Node archive checksum mismatch")
    execFileSync("tar", ["-xzf", file, "-C", cache])
    const distribution = join(cache, `node-v${version}-darwin-${process.arch}`)
    cpSync(join(distribution, "bin/node"), join(runtime, executable))
    cpSync(join(distribution, "LICENSE"), join(runtime, "NODE-LICENSE"))
    return version
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
