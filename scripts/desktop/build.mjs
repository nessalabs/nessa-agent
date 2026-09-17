import { spawnSync } from "node:child_process"
import { runDesktopBuild } from "./build-command.mjs"

const args = process.argv.slice(2)
const status = runDesktopBuild({
  args,
  environment: process.env,
  platform: process.platform,
  spawn: spawnSync,
})
if (status !== 0) process.exit(status)
