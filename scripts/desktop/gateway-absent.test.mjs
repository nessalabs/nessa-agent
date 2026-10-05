import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import test from "node:test"
import { gatewayAbsentNotice } from "./gateway-absent.mjs"

const sentences = JSON.parse(
  readFileSync(resolve("src/host/startup-refusals.json"), "utf8"),
)

test("a listening server needs no notice", () => {
  assert.equal(
    gatewayAbsentNotice({
      listening: true,
      port: 7421,
      stage: "dev",
      sentence: sentences["not-listening"],
    }),
    null,
  )
})

test("a quiet server names the sentence, the socket, and the stage", () => {
  const notice = gatewayAbsentNotice({
    listening: false,
    port: 7421,
    stage: "dev",
    sentence: sentences["not-listening"],
  })
  assert.match(notice, /not answering/)
  assert.match(notice, /127\.0\.0\.1:7421/)
  assert.match(notice, /stage dev/)
})

test("a notice without its sentence is refused", () => {
  assert.throws(() =>
    gatewayAbsentNotice({ listening: false, port: 1, stage: "dev", sentence: "" }),
  )
})

test("desktop dev names a quiet gateway and still starts", (context) => {
  const directory = mkdtempSync(join(tmpdir(), "nessa-gateway-absent-"))
  context.after(() => rmSync(directory, { recursive: true, force: true }))
  const pnpm = join(directory, "pnpm.mjs")
  writeFileSync(pnpm, "process.exit(0)\n")
  chmodSync(pnpm, 0o755)
  const result = spawnSync(process.execPath, [resolve("scripts/desktop/dev.mjs")], {
    encoding: "utf8",
    env: { ...process.env, npm_execpath: pnpm, NESSA_PORT: "9", NESSA_STAGE: "dev" },
  })
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stderr, /not answering/)
  assert.match(result.stderr, /127\.0\.0\.1:9/)
  assert.match(result.stderr, /stage dev/)
})
