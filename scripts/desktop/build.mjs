import { spawnSync } from "node:child_process"
import { runDesktopBuild } from "./build-command.mjs"

try {
  const status = runDesktopBuild({
    args: process.argv.slice(2),
    environment: process.env,
    platform: process.platform,
    spawn: spawnSync,
  })
  if (status !== 0) process.exit(status)
} catch (error) {
  console.error(`Desktop build did not start: ${error.message}`)
  process.exit(1)
}
