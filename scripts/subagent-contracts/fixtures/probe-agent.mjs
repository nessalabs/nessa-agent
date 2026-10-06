/** Test-only subprocess behavior; not a provider recording or a Nessa runtime. */
import { spawn } from "node:child_process"
import { readFileSync, writeFileSync } from "node:fs"
import { createInterface } from "node:readline"

const [scenario, file] = process.argv.slice(2)
if (scenario === "ignore-term") {
  process.on("SIGTERM", () => {})
  process.send?.({ ready: true })
  setInterval(() => {}, 1000)
} else if (scenario === "leader-first") {
  const descendant = spawn(process.execPath, [process.argv[1], "ignore-term"], {
    stdio: ["ignore", "ignore", "ignore", "ipc"],
  })
  descendant.once("message", () =>
    process.stdout.write(`${JSON.stringify({ descendant: descendant.pid })}\n`),
  )
  process.on("SIGTERM", () => process.exit(0))
  setInterval(() => {}, 1000)
} else if (scenario === "version-stall") {
  writeFileSync(file, String(process.pid))
  process.on("SIGTERM", () => {})
  setInterval(() => {}, 1000)
} else if (scenario === "version-ok") {
  process.stdout.write("codex-cli 0.154.0\n")
} else if (scenario.startsWith("oversized")) {
  process.stdout.write(
    `\"${"x".repeat(128 * 1024)}\"${scenario === "oversized-line" ? "\n" : ""}`,
  )
  setInterval(() => {}, 1000)
} else if (scenario.startsWith("invalid")) {
  process.stdout.write(
    `${JSON.stringify({ jsonrpc: "2.0", method: "_auth/status_update", params: { email: "SECRET_ACCOUNT", token: "SECRET_TOKEN" } })}\n`,
  )
  process.stdout.write(
    `${{ "invalid-null": "null", "invalid-array": "[]", "invalid-json": "{broken", "invalid-envelope": '{"jsonrpc":"2.0","method":2}' }[scenario]}\n`,
  )
  setInterval(() => {}, 1000)
} else {
  const captured = JSON.parse(
    readFileSync(new URL("./codex-native.json", import.meta.url), "utf8"),
  )
  const send = (id, result) =>
    process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id, result })}\n`)
  createInterface({ input: process.stdin }).on("line", (line) => {
    const request = JSON.parse(line)
    if (request.method === "initialize")
      send(request.id, {
        agentInfo: { name: "scripted-fixture", version: "1" },
        agentCapabilities: { sessionCapabilities: { close: {} } },
      })
    if (request.method === "session/new")
      send(request.id, {
        sessionId: "identity-1",
        modes: { currentModeId: "read-only" },
        configOptions: [{ id: "mode", currentValue: "read-only" }],
      })
    if (request.method === "session/prompt") {
      for (const { frame } of captured.frames) {
        if (!frame.params) continue
        const selected = structuredClone(frame)
        if (
          scenario === "child-error" &&
          selected.params.update.title === "wait" &&
          selected.params.update.sessionUpdate === "tool_call_update"
        )
          selected.params.update.rawInput.agentsStates["identity-3"].status = "errored"
        process.stdout.write(`${JSON.stringify(selected)}\n`)
      }
      send(request.id, {
        stopReason: scenario === "cancelled" ? "cancelled" : "end_turn",
      })
    }
    if (request.method === "session/close") send(request.id, {})
  })
}
