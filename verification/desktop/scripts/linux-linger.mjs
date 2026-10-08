#!/usr/bin/env node
/** Measures the Linux linger setup step in Chromium and WebKit. */
import assert from "node:assert/strict"
import { mkdirSync } from "node:fs"
import { join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { createServer } from "vite"
import { launch } from "./lib/browser.mjs"

const root = resolve(fileURLToPath(import.meta.url), "../../../..")
const shots = join(root, "verification/desktop/evidence/linux-linger")
mkdirSync(shots, { recursive: true })

const server = await createServer({
  logLevel: "warn",
  configFile: resolve(root, "verification/desktop/fixtures/linux-linger/vite.config.ts"),
})
const report = []

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
  const phases = [
    "offer-then-enable",
    "offer-then-decline",
    "already-enabled",
    "refused",
    "failed",
    "unsupported",
  ]
  for (const engine of ["chromium", "webkit"]) {
    const browser = await launch(engine, {
      channel: "bundled",
      headed: false,
      ...(engine === "webkit" ? webkitEnv() : {}),
    })
    try {
      for (const phase of phases) {
        const page = await browser.newPage({
          viewport: { width: 900, height: 900 },
          reducedMotion: "reduce",
        })
        const errors = []
        page.on("pageerror", (error) => errors.push(error.message))
        try {
          await page.goto(`http://127.0.0.1:${address.port}/linux-linger?phase=${phase}`)
          const skip = page.getByRole("button", { name: "Skip this step", exact: true })
          await skip.waitFor({ timeout: 10_000 })
          const skipBox = await skip.boundingBox()
          assert.ok(skipBox && skipBox.width > 0 && skipBox.height > 0)
          await skip.click()
          if (phase === "offer-then-decline") {
            const stay = page.getByRole("button", {
              name: "Only while I’m signed in",
              exact: true,
            })
            await stay.waitFor({ timeout: 10_000 })
            await stay.click()
            await page.locator("[data-step='done']").waitFor({ timeout: 10_000 })
            assert.equal(
              (await page.locator("body").innerText()).includes("keeps running"),
              false,
            )
          } else {
            const panel = page.locator("[data-linger]")
            await panel.waitFor({ timeout: 10_000 })
            const shown = await panel.getAttribute("data-linger")
            const claim = await panel.getAttribute("data-linger-claim")
            const text = (await panel.innerText()).replace(/\s+/g, " ")
            if (
              phase === "offer-then-enable" ||
              phase === "refused" ||
              phase === "failed"
            ) {
              assert.equal(shown, "offer")
              assert.equal(claim, "no")
              assert.equal(text.includes("keeps running"), false)
              assert.equal(text.includes("administrator"), false)
              const keep = page.getByRole("button", {
                name: "Keep it running",
                exact: true,
              })
              const keepBox = await keep.boundingBox()
              assert.ok(keepBox && keepBox.width > 0 && keepBox.height > 0)
              if (engine === "chromium" && phase === "offer-then-enable") {
                await page
                  .locator("[data-linger]")
                  .locator("..")
                  .screenshot({ path: join(shots, "offer.png") })
              }
            }
            if (phase === "offer-then-enable") {
              await page
                .getByRole("button", { name: "Keep it running", exact: true })
                .click()
              await page.getByText("Nessa keeps running when you log out").waitFor()
              assert.equal(
                await page.locator("[data-linger]").getAttribute("data-linger-claim"),
                "yes",
              )
              if (engine === "chromium") {
                await page
                  .locator("[data-linger]")
                  .locator("..")
                  .screenshot({ path: join(shots, "enabled.png") })
              }
            }
            if (phase === "already-enabled") {
              assert.equal(shown, "enabled")
              assert.equal(claim, "yes")
              assert.equal(text.includes("keeps running when you log out"), true)
            }
            if (phase === "refused" || phase === "failed") {
              await page
                .getByRole("button", { name: "Keep it running", exact: true })
                .click()
              await page.getByText("loginctl enable-linger").waitFor()
              assert.equal(
                await page.locator("[data-linger]").getAttribute("data-linger-claim"),
                "no",
              )
              assert.equal(
                (await page.locator("[data-linger]").innerText()).includes(
                  "keeps running",
                ),
                false,
              )
              if (engine === "chromium") {
                const name = phase === "refused" ? "refused.png" : "failed.png"
                await page
                  .locator("[data-linger]")
                  .locator("..")
                  .screenshot({ path: join(shots, name) })
              }
            }
            if (phase === "unsupported") {
              assert.equal(shown, "unsupported")
              assert.equal(claim, "no")
              assert.equal(text.includes("keeps running"), false)
              if (engine === "chromium") {
                await page
                  .locator("[data-linger]")
                  .locator("..")
                  .screenshot({ path: join(shots, "unsupported.png") })
              }
            }
          }
          const counts = await page.evaluate(() => window.__lingerProbe)
          if (
            phase === "already-enabled" ||
            phase === "unsupported" ||
            phase === "offer-then-decline"
          ) {
            assert.equal(counts.accepts, 0)
          }
          if (
            phase === "offer-then-enable" ||
            phase === "refused" ||
            phase === "failed"
          ) {
            assert.equal(counts.accepts, 1)
          }
          assert.deepEqual(errors, [])
          report.push({ engine, phase, accepts: counts.accepts, errors: errors.length })
        } finally {
          await page.close()
        }
      }
    } finally {
      await browser.close()
    }
  }
  process.stdout.write(
    `${JSON.stringify({ check: "linux-linger", passed: true, results: report }, null, 2)}\n`,
  )
} finally {
  await server.close()
}
