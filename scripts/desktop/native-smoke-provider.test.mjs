import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import test from "node:test"

test("the native smoke ACP fixture advertises and accepts image input", async () => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-native-provider-"))
  const entrypoint = join(process.cwd(), "scripts/desktop/native-smoke-provider.mjs")
  const provider = spawn(process.execPath, [entrypoint], {
    cwd: directory,
    env: { ...process.env, CODEX_CONFIG: JSON.stringify({ model: "test-model" }) },
    stdio: ["pipe", "pipe", "pipe"],
  })
  const stdout = []
  const stderr = []
  provider.stdout.on("data", (chunk) => stdout.push(chunk))
  provider.stderr.on("data", (chunk) => stderr.push(chunk))
  try {
    provider.stdin.end(
      [
        { jsonrpc: "2.0", id: 1, method: "initialize", params: {} },
        {
          jsonrpc: "2.0",
          id: 2,
          method: "session/prompt",
          params: {
            prompt: [
              { type: "text", text: "describe" },
              { type: "image", mimeType: "image/png", data: "AA==" },
            ],
          },
        },
      ]
        .map(JSON.stringify)
        .join("\n") + "\n",
    )
    const [code] = await new Promise((resolve) =>
      provider.once("close", (...args) => resolve(args)),
    )
    assert.equal(code, 0, Buffer.concat(stderr).toString())
    const messages = Buffer.concat(stdout).toString().trim().split("\n").map(JSON.parse)
    assert.equal(messages[0].result.agentCapabilities.promptCapabilities.image, true)
    assert.equal(
      messages[1].params.update.content.text,
      "Smoke reply: describe [1 image]",
    )
    assert.deepEqual(messages[2], {
      jsonrpc: "2.0",
      id: 2,
      result: { stopReason: "end_turn" },
    })
  } finally {
    if (provider.exitCode === null) provider.kill("SIGKILL")
    rmSync(directory, { recursive: true, force: true })
  }
})
