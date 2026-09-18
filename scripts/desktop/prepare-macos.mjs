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
// One harness per agent the gateway can start. A bundle missing any of them is
// an installation that cannot offer that agent, and the server says so on
// startup rather than after someone picks it.
const harnesses = {}
// Each harness and the one package it exists to pin. Named rather than taken
// from the manifest, because what goes into the runtime fingerprint has to be
// the version of a package we chose: reading "whichever dependency is listed
// first" would silently start fingerprinting something else the day a harness
// gains a second one.
const HARNESSES = {
  "claude-acp": "@agentclientprotocol/claude-agent-acp",
  "codex-acp": "@agentclientprotocol/codex-acp",
}
for (const [name, pinned] of Object.entries(HARNESSES)) {
  const harness = join(out, name)
  mkdirSync(harness, { recursive: true })
  for (const file of ["package.json", "package-lock.json"])
    cpSync(join(root, "crates/nessa-sdk/harnesses", name, file), join(harness, file))
  execFileSync("npm", ["ci", "--omit=dev", "--no-audit", "--no-fund"], {
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
// Ad-hoc sign nested executables for local distribution. Release signing remains Tauri's responsibility.
for (const name of ["node", "nessa", "nessa-mcp"])
  execFileSync("codesign", ["--force", "--sign", "-", join(out, name)])
const fingerprint = runtimeFingerprint(out)
writeFileSync(
  join(out, "manifest.json"),
  JSON.stringify(
    {
      node: version,
      claudeAcp: harnesses["claude-acp"],
      codexAcp: harnesses["codex-acp"],
      target: host,
      fingerprint,
    },
    null,
    2,
  ),
)
