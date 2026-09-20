import { spawnSync } from "node:child_process"
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join } from "node:path"

import { unreachableOffMacos } from "./desktop/platform-gates.mjs"

const tests = [
  "scripts/ensure-nessa-ui.test.mjs",
  "scripts/dev-agent-config.test.mjs",
  "scripts/e2e-verdict.test.mjs",
  "scripts/retrofit-conversation-agents.test.mjs",
  "scripts/gateway-port.test.mjs",
  ...readdirSync("scripts/desktop")
    .filter((name) => name.endsWith(".test.mjs"))
    .map((name) => `scripts/desktop/${name}`),
]
const config = spawnSync("node", ["--test", ...tests], {
  stdio: "inherit",
})
if (config.error) throw config.error
if (config.status !== 0) process.exit(config.status ?? 1)

const format = spawnSync("cargo", ["fmt", "-p", "nessa-app", "--", "--check"], {
  stdio: "inherit",
})
if (format.error) throw format.error
if (format.status !== 0) process.exit(format.status ?? 1)

// Runs on every platform, including the Linux image that stops short of the
// native build below. It reads source rather than compiling, which is the
// point: the compiler would answer this exactly, on a machine that builds for
// Windows, and there is no such machine here — so this is the one check in the
// file that says anything about the platforms this host is not.
const host = "src-tauri/src"
const sources = new Map()
const collect = (directory) => {
  for (const name of readdirSync(directory)) {
    const path = join(directory, name)
    if (statSync(path).isDirectory()) collect(path)
    else if (name.endsWith(".rs")) sources.set(path, readFileSync(path, "utf8"))
  }
}
collect(host)
const unreachable = unreachableOffMacos(sources, join(host, "main.rs"))
if (unreachable.length > 0) {
  console.error(
    "\n→ compiled on every platform, reachable only on macOS — `-D warnings` calls\n" +
      "  this dead code on Windows and Linux. Gate it the way its caller is gated,\n" +
      "  or give it a caller those platforms have:\n",
  )
  for (const found of unreachable)
    console.error(`    ${found.kind} ${found.name}  ${found.path}:${found.line}`)
  console.error(
    "\n  Reading source, not compiling: if one of these is reached through a macro\n" +
      "  or a trait object, say so where it is declared and spare it in\n" +
      "  scripts/desktop/platform-gates.mjs.\n",
  )
  process.exit(1)
}

/**
 * Whether this machine can build the host at all.
 *
 * Linux needs WebKitGTK and its headers, which a clone does not come with, so
 * this used to skip on Linux by name. That made the skip permanent: CI could
 * install the headers and would still have skipped, and a test whose Rust
 * behaviour differs on Linux — `Path` reads a backslash as an ordinary
 * character there, not a separator — had nowhere to fail. One did, and passed
 * on macOS by accident.
 *
 * Asking pkg-config instead makes the skip about what is missing rather than
 * about which platform this is. A developer without the headers gets the
 * sentence and the command; CI installs them and runs the tests.
 */
function canBuildTheHost() {
  if (process.platform !== "linux") return true
  const found = spawnSync("pkg-config", ["--exists", "webkit2gtk-4.1"])
  return !found.error && found.status === 0
}

if (!canBuildTheHost()) {
  console.log(
    "desktop host tests need WebKitGTK; check skipped\n" +
      "  sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev",
  )
  process.exit(0)
}

for (const args of [
  ["clippy", "-p", "nessa-app", "--all-targets", "--", "-D", "warnings"],
  ["test", "-p", "nessa-app"],
]) {
  const result = spawnSync("cargo", args, { stdio: "inherit" })
  if (result.error) throw result.error
  if (result.status !== 0) process.exit(result.status ?? 1)
}
