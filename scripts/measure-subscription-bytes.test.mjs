import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { once } from "node:events"
import { dirname, join } from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const script = join(
  dirname(fileURLToPath(import.meta.url)),
  "measure-subscription-bytes.mjs",
)

test("auth init failure shuts the fixture supervisor and the process exits", async () => {
  const child = spawn(process.execPath, ["--import", "tsx", script], {
    env: { ...process.env, NESSA_SMOKE_BINARY: "/bin/false" },
    stdio: ["ignore", "pipe", "pipe"],
  })
  let stderr = ""
  child.stderr.on("data", (chunk) => {
    stderr += chunk
  })
  const timer = setTimeout(() => child.kill("SIGKILL"), 5_000)
  const [code, signal] = await once(child, "exit")
  clearTimeout(timer)
  assert.equal(signal, null, `process stayed up after auth init failed: ${stderr}`)
  assert.equal(code, 1)
  assert.match(stderr, /auth init failed/)
})
