import { spawnSync } from "node:child_process"
import { desktopStageEnvironment, resolveDesktopStage } from "./stage.mjs"

try {
  const stage = resolveDesktopStage({ environment: process.env, fallback: "dev" })
  const result = spawnSync(
    process.platform === "win32" ? "pnpm.cmd" : "pnpm",
    ["exec", "tauri", "dev", ...process.argv.slice(2)],
    {
      env: desktopStageEnvironment(process.env, stage),
      stdio: "inherit",
    },
  )
  if (result.error) throw result.error
  process.exit(result.status ?? 1)
} catch (error) {
  console.error(`Desktop did not start: ${error.message}`)
  process.exit(1)
}
