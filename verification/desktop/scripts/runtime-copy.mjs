#!/usr/bin/env node
/** The agent list stays selectable while private-runtime copy progress moves. */
import assert from "node:assert/strict"
import { mkdirSync } from "node:fs"
import { join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { createServer } from "vite"
import { launch } from "./lib/browser.mjs"

const root = resolve(fileURLToPath(import.meta.url), "../../../..")
const shots = join(root, "verification/desktop/evidence/runtime-copy")
mkdirSync(shots, { recursive: true })

const server = await createServer({
  logLevel: "warn",
  configFile: resolve(root, "verification/desktop/fixtures/runtime-copy/vite.config.ts"),
})

function webkitEnv() {
  if (!process.env.NESSA_VERIFICATION_WEBKIT_LIBRARY_PATH) return {}
  return {
    env: {
      ...process.env,
      LD_LIBRARY_PATH: process.env.NESSA_VERIFICATION_WEBKIT_LIBRARY_PATH,
    },
  }
}

try {
  await server.listen()
  const address = server.httpServer.address()
  assert.equal(typeof address, "object")
  const report = []
  for (const engine of ["chromium", "webkit"]) {
    const browser = await launch(engine, {
      channel: "bundled",
      headed: false,
      ...(engine === "webkit" ? webkitEnv() : {}),
    })
    try {
      const page = await browser.newPage({
        viewport: { width: 900, height: 900 },
        reducedMotion: "reduce",
      })
      const errors = []
      page.on("pageerror", (error) => errors.push(error.message))
      await page.goto(`http://127.0.0.1:${address.port}/runtime-copy`)
      const claude = page.getByRole("button", { name: "Claude", exact: true })
      await claude.waitFor({ timeout: 10_000 })
      const bar = page.getByRole("progressbar", { name: "Preparing the private runtime" })
      await bar.waitFor({ timeout: 10_000 })
      assert.equal(await bar.getAttribute("aria-valuenow"), "0")
      assert.equal(await claude.getAttribute("aria-disabled"), null)
      const box = await claude.boundingBox()
      assert.ok(box && box.width > 0 && box.height > 0)
      if (engine === "chromium") {
        await page.screenshot({ path: join(shots, "copying.png") })
      }
      await page
        .getByRole("button", { name: "Advance runtime copy", exact: true })
        .click()
      await page.waitForFunction(
        () => document.querySelector("progress")?.getAttribute("aria-valuenow") === "50",
      )
      const advanced = await bar.getAttribute("aria-valuenow")
      assert.equal(advanced, "50")
      assert.equal(await claude.getAttribute("aria-disabled"), null)
      await page.getByRole("button", { name: "Finish runtime copy", exact: true }).click()
      await bar.waitFor({ state: "hidden", timeout: 10_000 })
      assert.equal(await claude.getAttribute("aria-disabled"), null)
      assert.deepEqual(errors, [])
      report.push({
        engine,
        copyingValue: 0,
        advancedValue: Number(advanced),
        settledBar: "hidden",
        claudeAriaDisabled: null,
        claudeWidth: box.width,
        claudeHeight: box.height,
        pageErrors: errors.length,
      })
    } finally {
      await browser.close()
    }
  }
  process.stdout.write(
    `${JSON.stringify({ check: "runtime-copy", passed: true, results: report })}\n`,
  )
} finally {
  await server.close()
}
