import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"
import net from "node:net"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { gatewayPort, selectedPort } from "../gateway-port.mjs"
import { gatewayAbsentNotice } from "./gateway-absent.mjs"
import { desktopStageEnvironment, resolveDesktopStage } from "./stage.mjs"

const sentences = JSON.parse(
  readFileSync(
    resolve(
      dirname(fileURLToPath(import.meta.url)),
      "../../src/host/startup-refusals.json",
    ),
    "utf8",
  ),
)

function portOpen(port) {
  return new Promise((resolveOpen) => {
    const socket = net.connect({ host: "127.0.0.1", port })
    const finish = (open) => {
      socket.destroy()
      resolveOpen(open)
    }
    socket.setTimeout(300)
    socket.once("connect", () => finish(true))
    socket.once("timeout", () => finish(false))
    socket.once("error", () => finish(false))
  })
}

try {
  const stage = resolveDesktopStage({ environment: process.env, fallback: "dev" })
  const packageManager = process.env.npm_execpath
  if (!packageManager) {
    throw new Error("Run this desktop command through `pnpm app`")
  }
  const override = (process.env.NESSA_PORT ?? "").trim()
  const port = override === "" ? gatewayPort(stage) : selectedPort(process.env)
  const sentence = Object.hasOwn(sentences, "not-listening")
    ? sentences["not-listening"]
    : ""
  const notice = gatewayAbsentNotice({
    listening: await portOpen(port),
    port,
    stage,
    sentence,
  })
  if (notice) console.error(notice)
  const result = spawnSync(
    process.execPath,
    [packageManager, "exec", "tauri", "dev", ...process.argv.slice(2)],
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
