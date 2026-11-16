import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { once } from "node:events"
import { mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import test from "node:test"
import { fileURLToPath, pathToFileURL } from "node:url"

const script = join(
  dirname(fileURLToPath(import.meta.url)),
  "measure-subscription-bytes.mjs",
)

test("auth init failure shuts the fixture supervisor and the process exits", async (context) => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-auth-init-failure-"))
  context.after(() => rmSync(directory, { recursive: true, force: true }))
  const fixture = join(directory, "fail-auth-init.mjs")
  // Node is the test-owned executable. Its preload silently refuses exactly
  // the gateway's auth-init argv, after the measurement supervisor is open.
  // The measurement process and other children run normally.
  writeFileSync(
    fixture,
    `
    import { basename } from "node:path"
    if (basename(process.argv[1] ?? "") === "auth" && process.argv[2] === "init")
      process.exit(1)
  `,
  )
  const child = spawn(process.execPath, ["--import", "tsx", script], {
    env: {
      ...process.env,
      NESSA_SMOKE_BINARY: process.execPath,
      NODE_OPTIONS: `${process.env.NODE_OPTIONS ?? ""} --import=${pathToFileURL(fixture).href}`,
    },
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
