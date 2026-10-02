/** Drives the actual panel's Enter and attachment gestures under controlled outside effects in both engines. */
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { launch } from "./lib/browser.mjs"
import { attachmentVerification as selectors } from "./lib/selectors.mjs"

const root = resolve(fileURLToPath(import.meta.url), "../../../..")
const url = "http://127.0.0.1:1448/attachment-verification"
const server = spawn(
  resolve(root, "node_modules/.bin/vite"),
  ["--config", "verification/desktop/fixtures/attachments-races/vite.config.ts"],
  { cwd: root, stdio: ["ignore", "pipe", "pipe"] },
)
let serverLog = ""
server.stdout.on("data", (chunk) => {
  serverLog += chunk
})
server.stderr.on("data", (chunk) => {
  serverLog += chunk
})
const report = []
const wait = async (predicate) => {
  const deadline = Date.now() + 30000
  while (Date.now() < deadline) {
    if (await predicate()) return
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  throw new Error(`Fixture server unavailable: ${serverLog.slice(-2000)}`)
}
try {
  await wait(async () => {
    try {
      return (await fetch(url)).ok
    } catch {
      return false
    }
  })
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
      for (const scenario of ["control", "native-read", "second-url"]) {
        const context = await browser.newContext({
          viewport: { width: 800, height: 900 },
          reducedMotion: "reduce",
        })
        const page = await context.newPage()
        const errors = []
        let refusalCount = 0
        let heldRead
        page.on("pageerror", (error) => errors.push(error.message))
        try {
          await page.goto(url, { waitUntil: "domcontentloaded", timeout: 120000 })
          await page.locator(selectors.editor).waitFor({ timeout: 120000 })
          await page.waitForFunction(
            () => typeof window.__attachmentProbe?.drop === "function",
          )
          const snapshot = () => page.evaluate(() => window.__attachmentProbe.snapshot())
          if (scenario === "control") {
            await page.locator(selectors.editor).press("Enter")
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().sends.length === 1,
            )
            assert.equal((await snapshot()).sends[0].text, "include this image")
          } else if (scenario === "native-read") {
            await page.getByRole(...selectors.addAttachment).click()
            await page.getByRole(...selectors.addFiles).click()
            await page
              .getByText(selectors.selectedName, { exact: true })
              .first()
              .waitFor()
            await page.locator(selectors.editor).press("Enter")
            await page.getByText(selectors.pendingNotice, { exact: true }).waitFor()
            refusalCount = await page
              .getByText(selectors.pendingNotice, { exact: true })
              .count()
            const held = await snapshot()
            heldRead = held
            assert.equal(held.sends.length, 0)
            assert.deepEqual(held.draft, [{ type: "text", text: "include this image" }])
            await page.evaluate(() => window.__attachmentProbe.completeRead())
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some((part) => part.type === "file"),
            )
            await page.locator(selectors.editor).press("Enter")
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().sends.length === 1,
            )
            const sent = (await snapshot()).sends[0]
            assert.equal(sent.text, "include this image")
            assert.equal(sent.attachments.length, 1)
          } else {
            for (const name of ["first", "second"]) {
              await page.evaluate(
                (name) =>
                  window.__attachmentProbe.drop({
                    batch: `browser-${name}`,
                    files: [],
                    refused: null,
                    text: {
                      plain: "",
                      uriList: `https://attachment-verification.invalid/${name}.png`,
                      html: `<img src="https://attachment-verification.invalid/${name}.png">`,
                    },
                  }),
                name,
              )
              if (name === "first")
                await page.waitForFunction(
                  () => window.__attachmentProbe.snapshot().fetchCalls === 1,
                )
            }
            await page.getByText(selectors.busyDropNotice, { exact: true }).waitFor()
            refusalCount = await page
              .getByText(selectors.busyDropNotice, { exact: true })
              .count()
            assert.equal((await snapshot()).fetchCalls, 1)
          }
          assert.deepEqual(errors, [])
          report.push({
            engine,
            scenario,
            status: "passed",
            sendCount: (await snapshot()).sends.length,
            attachmentCount: (await snapshot()).sends[0]?.attachments?.length ?? 0,
            refusalCount,
            ...(heldRead ? { heldRead } : {}),
            snapshot: await snapshot(),
            pageErrors: errors.length,
          })
        } catch (error) {
          process.stderr.write(
            JSON.stringify({
              engine,
              scenario,
              errors,
              body: (await page.locator("body").innerText()).slice(0, 1500),
              serverLog: serverLog.slice(-2000),
            }) + "\n",
          )
          throw error
        } finally {
          await context.close()
        }
      }
    } finally {
      await browser.close()
    }
  }
  process.stdout.write(
    JSON.stringify({ tests: report.length, passed: report.length, report }, null, 2) +
      "\n",
  )
} finally {
  server.kill()
  if (server.exitCode === null)
    await new Promise((resolve) => server.once("exit", resolve))
}
