/** Raw source provenance survives a Windows-style Git checkout. */
import { strict as assert } from "node:assert"
import { execFileSync } from "node:child_process"
import { createHash } from "node:crypto"
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { test } from "node:test"

const repo = fileURLToPath(new URL("../../", import.meta.url))
const files = [
  "scripts/subagent-contracts/capture.mjs",
  "scripts/subagent-contracts/acp-session.mjs",
  "scripts/subagent-contracts/processes.mjs",
  "scripts/subagent-contracts/evidence.mjs",
  "scripts/subagent-contracts/metadata.mjs",
  "crates/nessa-sdk/harnesses/codex-acp/package-lock.json",
  "scripts/mcp-test-server/server.mjs",
  "scripts/mcp-test-server/http-server.mjs",
  "scripts/mcp-test-server/capture-http.mjs",
]
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex")

test("hashed sources retain exact LF bytes with core.autocrlf true or false", (t) => {
  const root = mkdtempSync(join(tmpdir(), "nessa-provenance-checkout-"))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const git = (...args) =>
    execFileSync("git", args, {
      cwd: root,
      stdio: "pipe",
      timeout: 10000,
      env: {
        ...process.env,
        GIT_CONFIG_NOSYSTEM: "1",
        GIT_CONFIG_GLOBAL: process.platform === "win32" ? "NUL" : "/dev/null",
      },
    })
  git("init", "-q")
  for (const file of [".gitattributes", ...files]) {
    mkdirSync(dirname(join(root, file)), { recursive: true })
    writeFileSync(join(root, file), readFileSync(join(repo, file)))
  }
  git("-c", "core.autocrlf=false", "add", ".")
  git(
    "-c",
    "commit.gpgsign=false",
    "-c",
    "user.name=fixture",
    "-c",
    "user.email=fixture@example.invalid",
    "commit",
    "-qm",
    "fixture",
  )
  for (const autocrlf of ["true", "false"]) {
    for (const file of files) rmSync(join(root, file))
    git("-c", `core.autocrlf=${autocrlf}`, "checkout", "--", ...files)
    for (const file of files) {
      const actual = readFileSync(join(root, file))
      assert.equal(
        hash(actual),
        hash(readFileSync(join(repo, file))),
        `${autocrlf}: ${file}`,
      )
      assert.equal(actual.includes(Buffer.from("\r\n")), false, file)
    }
  }
})
