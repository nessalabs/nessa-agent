import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { createInterface } from "node:readline"
import test from "node:test"

function send(provider, value) {
  provider.stdin.write(`${JSON.stringify(value)}\n`)
}

test("the native smoke provider follows the pinned Claude image session contract", async () => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-native-provider-"))
  const entrypoint = join(process.cwd(), "scripts/desktop/native-smoke-provider.mjs")
  const provider = spawn(process.execPath, [entrypoint], {
    cwd: directory,
    env: {
      ...process.env,
      ANTHROPIC_MODEL: "claude-haiku-4-5-20251001",
      ANTHROPIC_CUSTOM_MODEL_OPTION: "claude-haiku-4-5-20251001",
      CLAUDE_CODE_MAX_OUTPUT_TOKENS: "4096",
    },
    stdio: ["pipe", "pipe", "pipe"],
  })
  const stderr = []
  provider.stderr.on("data", (chunk) => stderr.push(chunk))
  const messages = createInterface({ input: provider.stdout })[Symbol.asyncIterator]()
  async function receive() {
    const next = await messages.next()
    assert.equal(next.done, false, Buffer.concat(stderr).toString())
    return JSON.parse(next.value)
  }
  try {
    send(provider, { jsonrpc: "2.0", id: 1, method: "initialize", params: {} })
    const initialized = await receive()
    assert.equal(initialized.result.agentInfo.version, "0.76.0")
    assert.equal(initialized.result.agentCapabilities.promptCapabilities.image, true)
    assert.equal(initialized.result._meta.steering.supported, true)

    send(provider, {
      jsonrpc: "2.0",
      id: 2,
      method: "session/new",
      params: {
        cwd: directory,
        _meta: {
          claudeCode: {
            options: { model: "claude-haiku-4-5-20251001" },
          },
        },
      },
    })
    const opened = await receive()
    const sessionId = opened.result.sessionId
    assert.equal(typeof sessionId, "string")
    assert.deepEqual(opened.result.configOptions, [
      { id: "model", currentValue: "claude-haiku-4-5-20251001" },
      { id: "mode", currentValue: "default" },
    ])

    send(provider, {
      jsonrpc: "2.0",
      id: 3,
      method: "session/set_config_option",
      params: { sessionId, configId: "mode", value: "default" },
    })
    const configured = await receive()
    assert.deepEqual(configured.result.configOptions, opened.result.configOptions)

    send(provider, {
      jsonrpc: "2.0",
      id: 4,
      method: "session/prompt",
      params: {
        sessionId,
        prompt: [
          { type: "text", text: "describe" },
          { type: "image", mimeType: "image/png", data: "AA==" },
        ],
      },
    })
    const update = await receive()
    assert.equal(update.params.sessionId, sessionId)
    assert.equal(update.params.update.content.text, "Smoke reply: describe [1 image]")
    assert.deepEqual(await receive(), {
      jsonrpc: "2.0",
      id: 4,
      result: { stopReason: "end_turn" },
    })

    provider.stdin.end()
    const [code] = await new Promise((resolve) =>
      provider.once("close", (...args) => resolve(args)),
    )
    assert.equal(code, 0, Buffer.concat(stderr).toString())
  } finally {
    if (provider.exitCode === null) provider.kill("SIGKILL")
    rmSync(directory, { recursive: true, force: true })
  }
})
