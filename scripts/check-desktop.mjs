import { spawnSync } from "node:child_process"
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join } from "node:path"

import { unreachableOffMacos } from "./desktop/platform-gates.mjs"

const tests = [
  "scripts/ensure-nessa-ui.test.mjs",
  "scripts/dev-agent-config.test.mjs",
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

if (process.platform === "linux") {
  console.log(
    "desktop host tests require the native WebKit/GTK build image; check skipped",
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
