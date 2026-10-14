/** Drives the actual panel's Enter and attachment gestures under controlled outside effects in both engines. */
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { launch } from "./lib/browser.mjs"
import { attachmentVerification as selectors } from "./lib/selectors.mjs"

const root = resolve(fileURLToPath(import.meta.url), "../../../..")
const url = "http://127.0.0.1:1486/attachment-verification"
const scenarios = [
  "control",
  "native-read",
  "second-url",
  "upload-parent-success",
  "upload-parent-failure",
  "upload-placement-success",
  "upload-placement-failure",
  "shared-resource",
  "owner-swap",
  "detached-queue",
  "native-svg",
  "terminal-refusal-preserved",
  "terminal-refusal-reupload",
]
const only = process.argv[2] === "--only" ? process.argv[3]?.split(",") : undefined
if (process.argv.length > (only ? 4 : 2))
  throw new Error("Usage: attachments-races.mjs [--only scenario[,scenario...]]")
if (only?.some((scenario) => !scenarios.includes(scenario)))
  throw new Error(
    `Unknown attachment-races scenario: ${only?.find((scenario) => !scenarios.includes(scenario))}`,
  )
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
      for (const scenario of only ?? scenarios) {
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
          if (scenario.startsWith("terminal-refusal-")) {
            const reupload = scenario.endsWith("-reupload")
            await page.evaluate(
              (reupload) => window.__attachmentProbe.startTerminalRetention(reupload),
              reupload,
            )
            const uploadCountAfterFirst = reupload ? 2 : 1
            await page.waitForFunction(() => {
              const state = window.__attachmentProbe.snapshot()
              return (
                state.terminalRetention.stageSizes.length >= 1 &&
                state.draft.some(
                  (part) => part.type === "file" && part.upload.status === "stored",
                )
              )
            })
            const firstBlobUrl = (await snapshot()).terminalRetention.previewUrls[0]
            await page.locator(selectors.editor).press("Enter")
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().sends.length === 1,
            )
            const firstExecutionId = (await snapshot()).sends[0].executionId
            await page.waitForFunction((executionId) => {
              const state = window.__attachmentProbe.snapshot()
              return state.terminalRetention.turns?.some(
                (turn) => turn.executionId === executionId && turn.receipt === "failed",
              )
            }, firstExecutionId)
            await page.waitForFunction((expectedStages) => {
              const state = window.__attachmentProbe.snapshot()
              return (
                state.terminalRetention.stageSizes.length === expectedStages &&
                state.draft.some(
                  (part) => part.type === "file" && part.upload.status === "stored",
                )
              )
            }, uploadCountAfterFirst)
            await page.locator(selectors.editor).press("Enter")
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().sends.length === 2,
            )
            const retryExecutionId = (await snapshot()).sends[1].executionId
            await page.waitForFunction((executionId) => {
              const state = window.__attachmentProbe.snapshot()
              return (
                state.terminalRetention.acceptedSends === 1 &&
                state.terminalRetention.turns?.some(
                  (turn) =>
                    turn.executionId === executionId &&
                    ["accepted", "delivered"].includes(turn.receipt),
                )
              )
            }, retryExecutionId)
            await page.evaluate(() =>
              window.__attachmentProbe.attachSecondTerminalRetention(),
            )
            const expectedStageCount = reupload ? 3 : 2
            await page.waitForFunction((expectedStages) => {
              const state = window.__attachmentProbe.snapshot()
              return (
                state.terminalRetention.stageSizes.length === expectedStages &&
                state.draft.some(
                  (part) => part.type === "file" && part.upload.status === "stored",
                )
              )
            }, expectedStageCount)
            await page.locator(selectors.editor).press("Enter")
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().sends.length === 3,
            )
            const secondExecutionId = (await snapshot()).sends[2].executionId
            await page.waitForFunction((executionId) => {
              const state = window.__attachmentProbe.snapshot()
              return (
                state.terminalRetention.acceptedSends === 2 &&
                state.terminalRetention.retained.length === 2 &&
                state.terminalRetention.turns?.some(
                  (turn) =>
                    turn.executionId === executionId &&
                    ["accepted", "delivered"].includes(turn.receipt),
                )
              )
            }, secondExecutionId)
            const final = await snapshot()
            assert.deepEqual(
              final.terminalRetention.stageSizes,
              Array(expectedStageCount).fill(40 * 1024 * 1024),
            )
            assert.deepEqual(
              final.terminalRetention.imageSizes,
              Array(expectedStageCount).fill(final.terminalRetention.processedSize),
            )
            assert.ok(final.terminalRetention.processedSize < 40 * 1024 * 1024)
            assert.deepEqual(final.terminalRetention.retained, [false, true])
            assert.equal(final.terminalRetention.acceptedSends, 2)
            const failedTurn = final.terminalRetention.turns.find(
              (turn) => turn.executionId === firstExecutionId,
            )
            const retriedTurn = final.terminalRetention.turns.find(
              (turn) => turn.executionId === retryExecutionId,
            )
            const secondTurn = final.terminalRetention.turns.find(
              (turn) => turn.executionId === secondExecutionId,
            )
            assert.equal(failedTurn?.receipt, "failed")
            assert.deepEqual(failedTurn?.content, [
              {
                type: "image-reference",
                digest: final.terminalRetention.imageDigests[0],
              },
            ])
            assert.ok(["accepted", "delivered"].includes(retriedTurn?.receipt))
            assert.equal(retriedTurn?.content[0]?.type, "image-reference")
            assert.equal(
              retriedTurn?.content[0]?.digest,
              reupload
                ? final.terminalRetention.imageDigests[1]
                : final.terminalRetention.imageDigests[0],
            )
            assert.ok(["accepted", "delivered"].includes(secondTurn?.receipt))
            assert.deepEqual(
              secondTurn?.content.map((part) => part.type),
              ["file"],
            )
            assert.equal(secondTurn?.content[0]?.name, "second-retained.png")
            assert.equal(
              await page
                .locator("img")
                .evaluateAll(
                  (images, url) =>
                    images.some((image) => image.getAttribute("src") === url),
                  firstBlobUrl,
                ),
              false,
              "the evicted original preview URL remains in the rendered transcript",
            )
            if (reupload) {
              const unavailable = page.locator(
                '[data-image-reference][data-held-file="failed"]',
              )
              await unavailable.waitFor()
              assert.match(await unavailable.innerText(), /No longer kept/)
            }
            await page.screenshot({
              path: `/tmp/816-al24-${scenario}-${engine}.png`,
            })
          } else if (scenario === "native-svg") {
            await page.evaluate(() => window.__attachmentProbe.attachNativeSvg())
            const image = page
              .getByRole("button", { name: "Open native.svg", exact: true })
              .locator("img")
            await image.waitFor()
            await page.waitForFunction(() => {
              const image = document.querySelector("[data-upload] img")
              return (
                image instanceof HTMLImageElement &&
                image.complete &&
                image.naturalWidth > 0
              )
            })
            assert.equal(await image.evaluate((image) => image.naturalWidth), 4)
            await page.screenshot({ path: `/tmp/816-r3-native-svg-${engine}.png` })
          } else if (scenario === "detached-queue") {
            await page.evaluate(() => window.__attachmentProbe.startDetachedUploads())
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().detachedStageCount === 3,
            )
            await page.evaluate(() => window.__attachmentProbe.detachCoordinator())
            await page.locator(selectors.editor).waitFor({ state: "detached" })
            await page.evaluate(() => window.__attachmentProbe.finishDetachedUpload())
            await page.waitForFunction(
              () =>
                window.__attachmentProbe
                  .snapshot()
                  .draft.find((part) => part.type === "file")?.upload.status === "stored",
            )
            assert.equal((await snapshot()).detachedStageCount, 3)
            await page.evaluate(() => window.__attachmentProbe.remountCoordinator())
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().detachedStageCount === 4,
            )
            assert.equal(await page.locator(selectors.uploadTile).count(), 4)
            await page.screenshot({ path: `/tmp/816-r2-remount-${engine}.png` })
          } else if (scenario === "owner-swap") {
            await page.evaluate(() => window.__attachmentProbe.startOwnerUpload())
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().sharedUploads.length === 1,
            )
            await page
              .getByRole("button", { name: "Open original.png", exact: true })
              .click()
            await page
              .getByRole("dialog", { name: "original.png", exact: true })
              .waitFor()
            await page.screenshot({ path: `/tmp/816-r2-preview-original-${engine}.png` })
            await page.evaluate(() => window.__attachmentProbe.swapOwner())
            await page
              .getByRole("dialog", { name: "original.png", exact: true })
              .waitFor({ state: "detached" })
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().sharedUploads.length === 2,
            )
            await page.screenshot({
              path: `/tmp/816-r2-preview-replacement-${engine}.png`,
            })
            await page.evaluate(() => window.__attachmentProbe.finishOriginalOwner())
            assert.equal(
              (await snapshot()).draft.find((part) => part.type === "file").upload.status,
              "uploading",
            )
            assert.equal(
              await page.locator(selectors.uploadTile).getAttribute("data-upload"),
              "uploading",
            )
            await page.evaluate(() => window.__attachmentProbe.finishSharedUpload())
            await page.waitForFunction(
              () =>
                window.__attachmentProbe
                  .snapshot()
                  .draft.find((part) => part.type === "file")?.upload.status === "stored",
            )
            assert.equal(
              await page.locator(selectors.uploadTile).getAttribute("data-upload"),
              "stored",
            )
          } else if (scenario === "shared-resource") {
            await page.evaluate(() => window.__attachmentProbe.startSharedUploads())
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().sharedUploads.length === 2,
            )
            const shared = await snapshot()
            assert.equal(
              new Set(
                shared.tabs.flatMap((tab) => tab.files.map((file) => file.placementId)),
              ).size,
              2,
            )
            assert.equal(
              new Set(
                shared.tabs.flatMap((tab) =>
                  tab.files.map((file) => file.sharedResource),
                ),
              ).size,
              1,
            )
            await page.evaluate(() => window.__attachmentProbe.removeSharedParent())
            await page.waitForFunction(
              () =>
                window.__attachmentProbe
                  .snapshot()
                  .sharedUploads.filter((upload) => upload.aborted).length === 1,
            )
            assert.equal((await snapshot()).sharedBytesRetained, true)
            await page.evaluate(() => window.__attachmentProbe.finishSharedUpload())
            await page.waitForFunction(
              () =>
                window.__attachmentProbe.snapshot().draft[0]?.upload.status === "stored",
            )
            assert.equal(
              await page.locator(selectors.uploadTile).getAttribute("data-upload"),
              "stored",
            )
            await page.evaluate(() => window.__attachmentProbe.closeSharedParent())
            assert.equal((await snapshot()).sharedBytesRetained, false)
          } else if (
            scenario.startsWith("upload-parent-") ||
            scenario.startsWith("upload-placement-")
          ) {
            await page.evaluate(
              (sameParent) => window.__attachmentProbe.startParentUpload(sameParent),
              scenario.startsWith("upload-placement-"),
            )
            const tile = page.locator(selectors.uploadTile)
            await tile.waitFor()
            assert.equal(await tile.count(), 1)
            assert.equal(await tile.getAttribute("data-upload"), "uploading")
            await page.evaluate(
              (failed) => window.__attachmentProbe.finishParentUpload(failed),
              scenario.endsWith("-failure"),
            )
            assert.equal(await tile.getAttribute("data-upload"), "uploading")
            assert.equal(
              (await snapshot()).draft.find((part) => part.type === "file").upload.status,
              "uploading",
            )
          } else if (scenario === "control") {
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
              ...(scenario.startsWith("terminal-refusal-")
                ? {
                    snapshot: await page.evaluate(() =>
                      window.__attachmentProbe.snapshot(),
                    ),
                  }
                : {}),
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
