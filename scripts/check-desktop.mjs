import { spawnSync } from "node:child_process"
import { readdirSync } from "node:fs"

const tests = readdirSync("scripts/desktop")
  .filter((name) => name.endsWith(".test.mjs"))
  .map((name) => `scripts/desktop/${name}`)
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
