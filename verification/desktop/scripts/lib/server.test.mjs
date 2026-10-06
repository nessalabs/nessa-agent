/**
 * Preview cleanup, without building the app: a running preview is waited on
 * and its directory removed, and a production build that fails before it
 * starts a preview leaves no `nessa-desktop-verify-` directory behind.
 */
import { strict as assert } from "node:assert"
import { spawn } from "node:child_process"
import { existsSync, mkdtempSync, readdirSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import { pageMode, startPreview, stopPreview } from "./server.mjs"

const previewPrefix = "nessa-desktop-verify-"

test("stopping a preview waits for the process and removes its directory", async () => {
  const outDir = mkdtempSync(join(tmpdir(), "stop-preview-"))
  writeFileSync(join(outDir, "marker"), "kept")
  // Exit is delayed on purpose: a kill that is not awaited returns while this is still running.
  const preview = spawn(
    process.execPath,
    [
      "-e",
      "process.on('SIGTERM', () => setTimeout(() => process.exit(0), 150)); setInterval(() => {}, 1000); console.log('ready')",
    ],
    { stdio: ["ignore", "pipe", "ignore"] },
  )
  await new Promise((resolve) => preview.stdout.once("data", resolve))
  await stopPreview(outDir, preview)
  assert.equal(preview.exitCode, 0)
  assert.equal(existsSync(outDir), false)
})

test(
  "stopping a signal-killed preview again does not wait",
  { timeout: 3_000 },
  async () => {
    const outDir = mkdtempSync(join(tmpdir(), "stop-preview-"))
    const preview = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
      stdio: "ignore",
    })
    await stopPreview(outDir, preview)
    assert.equal(preview.signalCode, "SIGTERM")
    assert.equal(preview.exitCode, null)
    await stopPreview(outDir, preview)
    assert.equal(existsSync(outDir), false)
  },
)

test("stopping a preview that never started still removes its directory", async () => {
  const outDir = mkdtempSync(join(tmpdir(), "stop-preview-"))
  writeFileSync(join(outDir, "marker"), "kept")
  await stopPreview(outDir, undefined)
  assert.equal(existsSync(outDir), false)
})

test("a failed production build removes its directory", async () => {
  const before = new Set(
    readdirSync(tmpdir()).filter((name) => name.startsWith(previewPrefix)),
  )
  await assert.rejects(
    () =>
      startPreview(
        {},
        { NESSA_BROWSER_TLS_CERT: "cert-only", NESSA_BROWSER_TLS_KEY: "" },
      ),
    /vite build failed/,
  )
  const leaked = readdirSync(tmpdir()).filter(
    (name) => name.startsWith(previewPrefix) && !before.has(name),
  )
  assert.deepEqual(leaked, [])
})

test("an explicit --url keeps a named dev or prod mode and otherwise stays given", () => {
  const given = (mode) => (key) => key === "mode" && mode !== undefined
  assert.equal(
    pageMode({ url: "http://127.0.0.1:9/desktop.html", mode: "prod" }),
    "given",
  )
  assert.equal(
    pageMode({
      url: "http://127.0.0.1:9/desktop.html",
      mode: "prod",
      given: given("prod"),
    }),
    "prod",
  )
  assert.equal(
    pageMode({
      url: "http://127.0.0.1:9/desktop.html",
      mode: "dev",
      given: given("dev"),
    }),
    "dev",
  )
  assert.equal(pageMode({ mode: "prod", given: given("prod") }), "prod")
})
