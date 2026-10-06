#!/usr/bin/env node
/** Direct live provider probe. Existing sign-in; empty workspace; no product binding. */
import { spawn, spawnSync } from "node:child_process"
import { createHash } from "node:crypto"
import { once } from "node:events"
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { createInterface } from "node:readline"
import { fileURLToPath } from "node:url"
import { selectFrames, inspectFrames } from "./evidence.mjs"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..")
const harness = join(root, "crates/nessa-sdk/harnesses/codex-acp")
const entry = join(harness, "node_modules/@agentclientprotocol/codex-acp/dist/index.js")
const out = process.argv[2]
if (!out || !existsSync(entry)) {
  process.stderr.write(
    "capture.mjs <output.json> requires the installed pinned Codex ACP harness\n",
  )
  process.exit(2)
}
const workspace = mkdtempSync(join(tmpdir(), "nessa-subagent-contract-"))
const model = "gpt-5.6-luna"
const records = []
const MAX_RECORD_BYTES = 8 * 1024 * 1024
const MAX_RECORDS = 10_000
let recordedBytes = 0
let captureFailure
const pending = new Map()
let next = 1
const child = spawn(process.execPath, [entry], {
  cwd: workspace,
  env: {
    ...process.env,
    PATH: `${join(harness, "node_modules/.bin")}:${process.env.PATH}`,
    INITIAL_AGENT_MODE: "read-only",
    CODEX_CONFIG: JSON.stringify({ model, features: { multi_agent: true } }),
  },
  stdio: ["pipe", "pipe", "pipe"],
  detached: process.platform !== "win32",
})
const retain = (direction, frame) => {
  recordedBytes += Buffer.byteLength(JSON.stringify(frame))
  if (recordedBytes > MAX_RECORD_BYTES || records.length >= MAX_RECORDS) {
    captureFailure = { code: "recording_limit" }
    for (const item of pending.values()) {
      clearTimeout(item.timer)
      item.reject(captureFailure)
    }
    pending.clear()
    child.stdout.destroy()
    return false
  }
  records.push({ direction, frame })
  return true
}
const send = (frame) => {
  if (!retain("to-agent", frame)) return
  child.stdin.write(`${JSON.stringify(frame)}\n`)
}
const request = (method, params, budget = 45_000) => {
  if (captureFailure) return Promise.reject(captureFailure)
  const id = next++
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id)
      reject({ code: "timeout", method })
    }, budget)
    pending.set(id, { resolve, reject, timer })
    send({ jsonrpc: "2.0", id, method, params })
  })
}
child.stderr.resume()
child.stdin.on("error", () => {})
child.on("close", () => {
  for (const item of pending.values()) {
    clearTimeout(item.timer)
    item.reject({ code: "provider_exit" })
  }
  pending.clear()
})
child.on("error", () => {
  for (const item of pending.values()) {
    clearTimeout(item.timer)
    item.reject({ code: "spawn_failed" })
  }
  pending.clear()
})
createInterface({ input: child.stdout }).on("line", (line) => {
  let frame
  try {
    frame = JSON.parse(line)
  } catch {
    return
  }
  // Keep only in memory until allowlist selection, not a raw recorder file.
  if (!retain("from-agent", frame)) return
  if (frame.id !== undefined && !frame.method) {
    const item = pending.get(frame.id)
    if (item) {
      clearTimeout(item.timer)
      pending.delete(frame.id)
      item.resolve(frame)
    }
  } else if (frame.method === "session/request_permission") {
    send({ jsonrpc: "2.0", id: frame.id, result: { outcome: { outcome: "cancelled" } } })
  }
})
const hash = (path) => createHash("sha256").update(readFileSync(path)).digest("hex")
let source = {
  kind: "live-provider-acp",
  capturedAt: new Date().toISOString(),
  adapter: (() => {
    const packageInfo = JSON.parse(
      readFileSync(
        join(harness, "node_modules/@agentclientprotocol/codex-acp/package.json"),
        "utf8",
      ),
    )
    return `${packageInfo.name}@${packageInfo.version}`
  })(),
  adapterSha256: hash(entry),
  lockSha256: hash(join(harness, "package-lock.json")),
  probeSha256: hash(fileURLToPath(import.meta.url)),
  model,
  platform: `${process.platform}/${process.arch}`,
  declaredClientCapabilities: {
    subagents: {},
    fs: { readTextFile: false, writeTextFile: false },
    terminal: false,
  },
}
try {
  const initialized = await request("initialize", {
    protocolVersion: 1,
    clientInfo: { name: "nessa-subagent-probe", version: "1" },
    clientCapabilities: source.declaredClientCapabilities,
  })
  if (initialized.error)
    throw { code: "initialize_rejected", rpcCode: initialized.error.code }
  source.agentInfo = initialized.result.agentInfo
  source.sessionCapabilities = initialized.result.agentCapabilities.sessionCapabilities
  const opened = await request("session/new", { cwd: workspace, mcpServers: [] })
  if (opened.error) throw { code: "session_rejected", rpcCode: opened.error.code }
  source.initialMode = opened.result.modes.currentModeId
  source.modeConfig = opened.result.configOptions.find((option) => option.id === "mode")
  const sessionId = opened.result.sessionId
  const prompt =
    "Use your native subagent capability to spawn one child agent. Tell the child to reply with exactly CHILD_DONE without tools, files, network, or commands. Wait for the child to finish, close it if a close operation is available, then reply exactly PARENT_DONE. Do not perform any other actions."
  source.prompt = prompt
  const result = await request(
    "session/prompt",
    { sessionId, prompt: [{ type: "text", text: prompt }] },
    90_000,
  )
  if (result.error) throw { code: "prompt_rejected", rpcCode: result.error.code }
  const close = await request("session/close", { sessionId })
  source.parentClose = close.error
    ? { kind: "rpc_error", code: close.error.code }
    : { kind: "rpc_response", result: close.result }
  source.permissionRequests = records.filter(
    ({ frame }) => frame.method === "session/request_permission",
  ).length
  source.nativeSessionUpdates = records.filter(({ frame }) =>
    frame.params?.update?.sessionUpdate?.startsWith("subagent"),
  ).length
  const frames = selectFrames(records)
  const verdict = inspectFrames(frames)
  const provider = spawnSync(join(harness, "node_modules/.bin/codex"), ["--version"], {
    encoding: "utf8",
  })
  source.providerVersion = /^codex-cli [0-9][\w.+-]*\s*$/.test(provider.stdout)
    ? provider.stdout.trim()
    : "unavailable"
  writeFileSync(
    out,
    `${JSON.stringify({ source, outcome: { kind: "completed", ...verdict }, frames }, null, 2)}\n`,
  )
  process.stdout.write(
    `${JSON.stringify({ kind: "completed", tools: verdict.tools, nativeSessionUpdates: source.nativeSessionUpdates, permissionRequests: source.permissionRequests })}\n`,
  )
} catch (error) {
  writeFileSync(
    out,
    `${JSON.stringify({ source, outcome: { kind: "failed", code: error.code ?? "probe_failed", ...(error.rpcCode === undefined ? {} : { rpcCode: error.rpcCode }) }, frames: [] }, null, 2)}\n`,
  )
  process.stdout.write(
    `${JSON.stringify({ kind: "failed", code: error.code ?? "probe_failed" })}\n`,
  )
  process.exitCode = 1
} finally {
  for (const item of pending.values()) clearTimeout(item.timer)
  const exit = once(child, "close").catch(() => {})
  try {
    process.platform === "win32"
      ? child.kill("SIGTERM")
      : process.kill(-child.pid, "SIGTERM")
  } catch {}
  const kill = setTimeout(() => {
    try {
      process.platform === "win32"
        ? child.kill("SIGKILL")
        : process.kill(-child.pid, "SIGKILL")
    } catch {}
  }, 2000)
  if (child.exitCode === null && child.signalCode === null) await exit
  clearTimeout(kill)
  rmSync(workspace, { recursive: true, force: true })
}
