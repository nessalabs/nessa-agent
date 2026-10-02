#!/usr/bin/env node
/** Drives setup's timed-out real HTTP request and retry in Chromium and WebKit. */
import assert from "node:assert/strict"
import { resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { createServer } from "vite"
import { launch } from "./lib/browser.mjs"
import { readinessVerification } from "./lib/selectors.mjs"

const root = resolve(fileURLToPath(import.meta.url), "../../../..")
const server = await createServer({
  logLevel: "warn",
  configFile: resolve(
    root,
    "verification/desktop/fixtures/onboarding-readiness/vite.config.ts",
  ),
})
const report = []
try {
  await server.listen()
  const address = server.httpServer.address()
  assert.equal(typeof address, "object")
  for (const engine of ["chromium", "webkit"]) {
    const browser = await launch(engine, {
      channel: "bundled",
      headed: false,
      ...(engine === "webkit" && process.env.NESSA_VERIFICATION_WEBKIT_LIBRARY_PATH
        ? {
            env: {
              ...process.env,
              LD_LIBRARY_PATH: process.env.NESSA_VERIFICATION_WEBKIT_LIBRARY_PATH,
            },
          }
        : {}),
    })
    try {
      for (const phase of ["fetch", "body"]) {
        const page = await browser.newPage({
          viewport: { width: 900, height: 900 },
          reducedMotion: "reduce",
        })
        const errors = []
        page.on("pageerror", (error) => errors.push(error.message))
        try {
          const token = `${engine}-${phase}`
          await page.goto(
            `http://127.0.0.1:${address.port}/onboarding-readiness?token=${token}&phase=${phase}`,
          )
          const checking = page.getByRole("button", {
            name: readinessVerification.checkingButton,
            exact: true,
          })
          try {
            await checking.waitFor({ timeout: 5_000 })
          } catch (error) {
            const observed = await page.evaluate(
              (selectors) => ({
                text: document.body.textContent?.slice(0, 1500),
                buttons: [...document.querySelectorAll(selectors.buttons)].map(
                  (button) => ({
                    text: button.textContent,
                    disabled: button.disabled,
                    rect: button.getBoundingClientRect().toJSON(),
                  }),
                ),
                probe: window.__readinessProbe?.snapshot(),
              }),
              readinessVerification,
            )
            process.stderr.write(
              `${JSON.stringify({ engine, phase, errors, observed })}\n`,
            )
            throw error
          }
          assert.equal(await checking.isDisabled(), true)
          const retry = page.getByRole("button", {
            name: readinessVerification.retryButton,
            exact: true,
          })
          await retry.waitFor({ timeout: 13_000 })
          assert.equal(await retry.isEnabled(), true)
          const before = await page.evaluate(() => window.__readinessProbe.snapshot())
          assert.equal(before.requests.length, 1)
          assert.equal(before.requests[0].aborted, true)
          const elapsedMs = before.checkedAt - before.requests[0].started
          assert.ok(
            elapsedMs >= 9_900 && elapsedMs <= 12_000,
            `readiness released at ${elapsedMs}ms`,
          )
          const rect = await retry.boundingBox()
          assert.ok(
            rect?.width > 0 && rect?.height > 0,
            "retry control has no painted bounds",
          )
          await retry.click()
          await page
            .getByText(readinessVerification.readyObservation, { exact: true })
            .waitFor()
          const after = await page.evaluate(() => window.__readinessProbe.snapshot())
          assert.equal(after.requests.length, 2)
          assert.equal(after.requests[1].aborted, false)
          assert.deepEqual(errors, [])
          report.push({
            engine,
            phase,
            elapsedMs,
            retryRect: rect,
            requests: after.requests.length,
            errors: errors.length,
          })
        } finally {
          await page.close()
        }
      }
    } finally {
      await browser.close()
    }
  }
  process.stdout.write(
    `${JSON.stringify({ check: "onboarding-readiness", passed: true, results: report }, null, 2)}\n`,
  )
} finally {
  await server.close()
}
