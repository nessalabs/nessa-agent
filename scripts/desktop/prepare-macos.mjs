// Build a relocatable, self-contained runtime. No user config or credentials enter the bundle.
import { execFileSync } from "node:child_process"
import {
  mkdirSync,
  cpSync,
  readFileSync,
  writeFileSync,
  existsSync,
  rmSync,
} from "node:fs"
import { resolve, join } from "node:path"
import { createHash } from "node:crypto"
import { runtimeFingerprint } from "./runtime-fingerprint.mjs"
import { materializeBinLinks } from "./materialize-bin-links.mjs"
import {
  RUNTIME_EXECUTABLES,
  runtimeEntitlements,
  signingArguments,
} from "./runtime-signing.mjs"
const root = resolve(import.meta.dirname, "../..")
if (process.platform !== "darwin")
  throw new Error("Bundled gateway packaging currently supports macOS")
const target = process.env.TAURI_ENV_TARGET_TRIPLE
const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(
  /^host: (.+)$/m,
)[1]
if (target && target !== host)
  throw new Error("Build the desktop runtime on the target architecture")
const out = join(root, "src-tauri/runtime")
execFileSync("cargo", ["build", "--release", "-p", "nessa-server", "-p", "nessa-mcp"], {
  cwd: root,
  stdio: "inherit",
})
const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: root,
  }),
)
// Rebuild the shipped tree so removed resources cannot survive another build.
rmSync(out, { recursive: true, force: true })
mkdirSync(out, { recursive: true })
for (const name of ["nessa", "nessa-mcp"])
  cpSync(join(metadata.target_directory, "release", name), join(out, name))
const version = "26.8.1"
const archive = `node-v${version}-darwin-${process.arch}.tar.gz`
const cache = join(root, "target/desktop-downloads")
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
cpSync(
  join(cache, `node-v${version}-darwin-${process.arch}`, "bin/node"),
  join(out, "node"),
)
cpSync(
  join(cache, `node-v${version}-darwin-${process.arch}`, "LICENSE"),
  join(out, "NODE-LICENSE"),
)
const harness = join(out, "claude-acp")
mkdirSync(harness, { recursive: true })
for (const name of ["package.json", "package-lock.json"])
  cpSync(join(root, "crates/nessa-sdk/harnesses/claude-acp", name), join(harness, name))
execFileSync("npm", ["ci", "--omit=dev", "--no-audit", "--no-fund"], {
  cwd: harness,
  stdio: "inherit",
})
materializeBinLinks(join(harness, "node_modules"))
cpSync(join(root, "crates/nessa-sdk/data/models.json"), join(out, "models.json"))
// Sign the nested executables. Not Tauri's responsibility, whatever the comment
// that used to be here said: the bundler signs the app and `Contents/MacOS`,
// and treats a resource as a file, so these ship with whatever signature they
// are given at this point. Ad-hoc is fine for a build that stays on this
// machine and is what Apple rejected v0.1.0 for — see runtime-signing.mjs.
const identity = process.env.APPLE_SIGNING_IDENTITY?.trim() || undefined
for (const name of RUNTIME_EXECUTABLES) {
  const plist = runtimeEntitlements(name)
  execFileSync(
    "codesign",
    signingArguments(join(out, name), {
      identity,
      entitlements: plist ? join(root, "src-tauri", plist) : undefined,
    }),
    { stdio: "inherit" },
  )
}
process.stdout.write(
  identity
    ? `→ runtime executables signed with ${identity}, hardened and timestamped\n`
    : "→ runtime executables signed ad-hoc; this bundle cannot be notarized\n",
)
const fingerprint = runtimeFingerprint(out)
writeFileSync(
  join(out, "manifest.json"),
  JSON.stringify(
    {
      node: version,
      claudeAcp: "0.76.0",
      target: host,
      fingerprint,
    },
    null,
    2,
  ),
)
