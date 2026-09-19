import { spawnSync } from "node:child_process"
import { readdirSync } from "node:fs"

const tests = [
  "scripts/ensure-nessa-ui.test.mjs",
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
