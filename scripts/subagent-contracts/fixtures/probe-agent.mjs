/** Test-only subprocess behavior; not a provider recording or a Nessa runtime. */
import { spawn } from "node:child_process"
import { readFileSync, writeFileSync } from "node:fs"
import { createInterface } from "node:readline"

const [scenario, file] = process.argv.slice(2)
if (scenario === "hold-prompt" || scenario === "cleanup-hold") {
  process.on("SIGTERM", () => {
    writeFileSync(file, JSON.stringify({ phase: "term", pid: process.pid }))
  })
  setInterval(() => {}, 1000)
}
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
    if (request.method === "initialize") {
      if (scenario === "hold-initialize") {
        writeFileSync(file, JSON.stringify({ phase: "started", pid: process.pid }))
        return
      }
      if (scenario === "startup-native")
        process.stdout.write(`${JSON.stringify(captured.frames[0].frame)}\n`)
      const agentInfo = { name: "scripted-fixture", version: "1" }
      const sessionCapabilities = { close: {} }
      if (scenario === "metadata-secrets") {
        agentInfo.account = "SECRET_ACCOUNT"
        agentInfo.title = "SECRET_TOKEN"
        agentInfo.path = "SECRET_PATH"
        sessionCapabilities.account = "SECRET_ACCOUNT"
        sessionCapabilities.close = { token: "SECRET_TOKEN", path: "SECRET_PATH" }
      }
      if (scenario === "metadata-invalid-agent") agentInfo.name = "SECRET_ACCOUNT"
      send(request.id, { agentInfo, agentCapabilities: { sessionCapabilities } })
    }
    if (request.method === "session/new") {
      const result = {
        sessionId: "identity-1",
        modes: { currentModeId: "read-only" },
        configOptions: [{ id: "mode", currentValue: "read-only" }],
      }
      if (scenario === "metadata-secrets") {
        result.modes.account = "SECRET_ACCOUNT"
        result.configOptions[0] = {
          id: "mode",
          currentValue: "read-only",
          name: "SECRET_ACCOUNT",
          description: "SECRET_TOKEN",
          _meta: { token: "SECRET_TOKEN" },
          options: [
            {
              value: "read-only",
              name: "SECRET_ACCOUNT",
              description: "SECRET_PATH",
              _meta: { kind: "standard", token: "SECRET_TOKEN" },
            },
          ],
        }
      }
      if (scenario === "metadata-invalid-mode")
        result.modes.currentModeId = "SECRET_TOKEN"
      if (scenario === "open-before-prompt-native")
        process.stdout.write(
          `${JSON.stringify({ jsonrpc: "2.0", id: request.id, result })}\n${JSON.stringify(captured.frames[0].frame)}\n`,
        )
      else send(request.id, result)
    }
    if (request.method === "session/prompt") {
      if (scenario === "hold-prompt") {
        writeFileSync(file, JSON.stringify({ phase: "prompt", pid: process.pid }))
        return
      }
      if (file && scenario.startsWith("metadata-"))
        writeFileSync(file, "prompt-dispatched")
      for (const { frame } of captured.frames) {
        if (!frame.params) continue
        const selected = structuredClone(frame)
        if (scenario === "foreign-session") {
          selected.params.sessionId = "foreign-parent"
          selected.params.update.rawInput.senderThreadId = "foreign-parent"
          selected.params.update._meta.codex.collaboration.senderThreadId =
            "foreign-parent"
        }
        if (
          scenario === "child-error" &&
          selected.params.update.title === "wait" &&
          selected.params.update.sessionUpdate === "tool_call_update"
        )
          selected.params.update.rawInput.agentsStates["identity-3"].status = "errored"
        process.stdout.write(`${JSON.stringify(selected)}\n`)
      }
      if (["forged-terminal", "forged-terminal-only"].includes(scenario))
        send(999, { stopReason: "end_turn" })
      if (scenario === "forged-terminal-only") return
      send(
        request.id,
        ["forged-terminal", "empty-terminal"].includes(scenario)
          ? {}
          : {
              stopReason: scenario === "cancelled" ? "cancelled" : "end_turn",
            },
      )
      if (scenario === "terminal-trailing-native")
        process.stdout.write(`${JSON.stringify(captured.frames[0].frame)}\n`)
    }
    if (request.method === "session/close") {
      if (scenario === "cleanup-hold") {
        process.stdout.write(
          `${JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32000, message: "SECRET_DENIAL" } })}\n`,
        )
        return
      }
      const reply = JSON.stringify({ jsonrpc: "2.0", id: request.id, result: {} })
      if (scenario === "close-eof") process.stdout.end(reply, () => process.exit(0))
      else if (scenario === "close-null") process.stdout.write(`${reply}\nnull\n`)
      else if (scenario === "close-partial") process.stdout.write(`${reply}\nnull`)
      else if (["close-rejected", "close-rejected-null"].includes(scenario))
        process.stdout.write(
          `${JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32000, message: "SECRET_DENIAL" } })}\n${scenario === "close-rejected-null" ? "null\n" : ""}`,
        )
      else send(request.id, {})
      if (scenario === "late-null")
        setTimeout(
          () =>
            process.stdout.write(
              'null\n{"jsonrpc":"2.0","method":"_auth/status_update","params":{"email":"SECRET_LATE"}}\n',
            ),
          25,
        )
    }
  })
}
