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
  "picker-owner-correlation",
  "folder-owner-send",
  "folder-owner-drop",
  "drop-capacity",
  "conversation-refusals",
  "pathless-host-drop",
  "browser-picker-fallback",
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
          } else if (scenario === "picker-owner-correlation") {
            await page.evaluate(() => (window.__attachmentProbe.holdPickers = true))
            await page.getByRole(...selectors.addAttachment).click()
            await page.getByRole(...selectors.addFiles).click()
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().pickerRequestIds.length === 1,
            )
            const idA = (await snapshot()).pickerRequestIds[0]

            await page.evaluate(() => window.__attachmentProbe.swapPickerOwner())
            // The replacement must be committed before the next gesture. The
            // upload-owner fixture seeds an image, creates a server conversation,
            // and changes this tray's name to More options; it is not this setup.
            await page
              .locator('[data-attachment-owner="replacement"]')
              .waitFor({ state: "attached" })
            assert.deepEqual((await snapshot()).draft, [])
            await page.getByRole(...selectors.addAttachment).click()
            await page.getByRole(...selectors.addFiles).click()
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().pickerRequestIds.length === 2,
            )
            const idB = (await snapshot()).pickerRequestIds.find((id) => id !== idA)
            assert.ok(idA && idB && idA !== idB)

            // Deliver B's readiness first, then A's. B is the visible owner;
            // neither event may be assigned by whichever picker was latest.
            await page.evaluate(
              ({ idA, idB }) => {
                window.__attachmentProbe.announcePick(idB, "b.pdf")
                window.__attachmentProbe.announcePick(idA, "a.pdf")
              },
              { idA, idB },
            )
            await page.waitForFunction(() =>
              document.querySelector('[aria-label="Getting b.pdf ready"]'),
            )
            assert.equal(
              await page.locator('[aria-label="Getting b.pdf ready"]').count(),
              1,
            )
            assert.equal(
              await page.locator('[aria-label="Getting a.pdf ready"]').count(),
              0,
            )

            await page.evaluate(() => window.__attachmentProbe.showOriginalOwner())
            await page.waitForFunction(() =>
              document.querySelector('[aria-label="Getting a.pdf ready"]'),
            )
            assert.equal(
              await page.locator('[aria-label="Getting a.pdf ready"]').count(),
              1,
            )
            assert.equal(
              await page.locator('[aria-label="Getting b.pdf ready"]').count(),
              0,
            )

            const chosen = (name) => [
              {
                path: `/verification/${name}`,
                name,
                size: 1,
                mimeType: "application/pdf",
                ticket: `ticket-${name}`,
              },
            ]
            await page.evaluate(
              ({ idB, files }) => window.__attachmentProbe.settlePick(idB, files),
              { idB, files: chosen("b.pdf") },
            )
            await page.evaluate(() => window.__attachmentProbe.showReplacementOwner())
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some(
                  (part) =>
                    part.type === "file" && part.resource.path === "/verification/b.pdf",
                ),
            )
            assert.equal(
              (await snapshot()).draft.filter(
                (part) =>
                  part.type === "file" && part.resource.path === "/verification/b.pdf",
              ).length,
              1,
            )

            await page.evaluate(() => window.__attachmentProbe.showOriginalOwner())
            assert.equal(
              (await snapshot()).draft.some(
                (part) =>
                  part.type === "file" && part.resource.path === "/verification/b.pdf",
              ),
              false,
            )
            await page.evaluate(
              ({ idA, files }) => window.__attachmentProbe.settlePick(idA, files),
              { idA, files: chosen("a.pdf") },
            )
            await page.evaluate(() => window.__attachmentProbe.showOriginalOwner())
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some(
                  (part) =>
                    part.type === "file" && part.resource.path === "/verification/a.pdf",
                ),
            )
            assert.equal(
              (await snapshot()).draft.filter(
                (part) =>
                  part.type === "file" && part.resource.path === "/verification/a.pdf",
              ).length,
              1,
            )
          } else if (scenario === "folder-owner-send") {
            await page.evaluate(() =>
              window.__attachmentProbe.dropFolder("folder-a", true),
            )
            await page.waitForFunction(() =>
              window.__attachmentProbe.snapshot().folderStarts.includes("folder-a"),
            )
            await page.evaluate(() => window.__attachmentProbe.swapPickerOwner())
            await page
              .locator('[data-attachment-owner="replacement"]')
              .waitFor({ state: "attached" })

            // Keep the mounted editor's actual text while the store/resources
            // provider changes; Enter is the next user action after commit.
            await page.locator(selectors.editor).press("Enter")
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().sends.length === 1,
            )
            assert.equal((await snapshot()).sends[0].text, "include this image")
            assert.deepEqual((await snapshot()).draft, [])
            await page.screenshot({
              path: resolve(
                root,
                `verification/desktop/evidence/attachment-gesture-owners/folder-owner-send-b-${engine}.png`,
              ),
            })

            await page.evaluate(() => window.__attachmentProbe.finishFolder("folder-a"))
            await page.evaluate(() => window.__attachmentProbe.showOriginalOwner())
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some(
                  (part) => part.type === "file" && part.resource.name === "folder-a.txt",
                ),
            )
            await page.screenshot({
              path: resolve(
                root,
                `verification/desktop/evidence/attachment-gesture-owners/folder-owner-send-a-${engine}.png`,
              ),
            })
          } else if (scenario === "folder-owner-drop") {
            await page.evaluate(() =>
              window.__attachmentProbe.dropFolder("folder-a", true),
            )
            await page.waitForFunction(() =>
              window.__attachmentProbe.snapshot().folderStarts.includes("folder-a"),
            )
            await page.evaluate(() => window.__attachmentProbe.swapPickerOwner())
            await page
              .locator('[data-attachment-owner="replacement"]')
              .waitFor({ state: "attached" })

            // The second folder gesture arrives on the first committed render
            // for B, while A's reader still owns the only pending operation.
            await page.evaluate(() =>
              window.__attachmentProbe.dropFolder("folder-b", false),
            )
            await page.waitForFunction(() =>
              window.__attachmentProbe.snapshot().folderStarts.includes("folder-b"),
            )
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some(
                  (part) => part.type === "file" && part.resource.name === "folder-b.txt",
                ),
            )
            await page.screenshot({
              path: resolve(
                root,
                `verification/desktop/evidence/attachment-gesture-owners/folder-owner-drop-b-${engine}.png`,
              ),
            })

            await page.evaluate(() => window.__attachmentProbe.finishFolder("folder-a"))
            await page.evaluate(() => window.__attachmentProbe.showOriginalOwner())
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some(
                  (part) => part.type === "file" && part.resource.name === "folder-a.txt",
                ),
            )
            await page.screenshot({
              path: resolve(
                root,
                `verification/desktop/evidence/attachment-gesture-owners/folder-owner-drop-a-${engine}.png`,
              ),
            })
          } else if (scenario === "drop-capacity") {
            const dropFile = (name) => ({
              path: `/verification/${name}`,
              name,
              size: 1,
              mimeType: "application/pdf",
              ticket: `ticket-${name}`,
            })
            const deliver = async (batch, result) =>
              page.evaluate(
                ({ batch, result }) =>
                  window.__attachmentProbe.drop({
                    batch,
                    files: result.files ?? [],
                    refused: result.refused ?? null,
                    text: { plain: "", uriList: "", html: "" },
                  }),
                { batch, result },
              )

            await page.evaluate(() => {
              for (let index = 0; index < 64; index++)
                window.__attachmentProbe.began?.(`capacity-${index}`, "dropped")
            })
            assert.equal(
              await page.getByText("Too many selections", { exact: true }).count(),
              0,
            )
            await page.evaluate(() => window.__attachmentProbe.swapPickerOwner())
            await page
              .locator('[data-attachment-owner="replacement"]')
              .waitFor({ state: "attached" })
            assert.deepEqual((await snapshot()).draft, [])

            await page.evaluate(() =>
              window.__attachmentProbe.began?.("capacity-overflow", "dropped"),
            )
            await page.getByText("Too many selections", { exact: true }).waitFor()
            refusalCount = await page
              .getByText("Too many selections", { exact: true })
              .count()
            assert.equal(refusalCount, 1)
            await page.screenshot({
              path: `/tmp/attachment-drop-capacity-refusal-${engine}.png`,
            })

            // A rejected overflow result and an unrelated host result have no
            // captured destination. Both are ignored while B/c0 is visible.
            await deliver("capacity-overflow", {
              refused: { reason: "folder-empty", shown: null, detail: null },
            })
            await deliver("capacity-unknown", { files: [dropFile("unknown.pdf")] })
            assert.deepEqual((await snapshot()).draft, [])
            assert.equal(
              await page.getByText("Too many selections", { exact: true }).count(),
              1,
            )

            await page.evaluate(() => window.__attachmentProbe.showOriginalOwner())
            assert.deepEqual((await snapshot()).draft, [
              { type: "text", text: "include this image" },
            ])
            await deliver("capacity-0", { files: [dropFile("oldest.pdf")] })
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some(
                  (part) =>
                    part.type === "file" &&
                    part.resource.path === "/verification/oldest.pdf",
                ),
            )
            await page
              .getByRole("button", { name: "Open oldest.pdf", exact: true })
              .waitFor()
            await deliver("capacity-0", { files: [dropFile("duplicate.pdf")] })
            const original = await snapshot()
            assert.equal(
              original.draft.filter(
                (part) =>
                  part.type === "file" &&
                  part.resource.path === "/verification/oldest.pdf",
              ).length,
              1,
            )
            assert.equal(
              original.draft.some(
                (part) =>
                  part.type === "file" &&
                  part.resource.path === "/verification/duplicate.pdf",
              ),
              false,
            )
            await page.screenshot({
              path: `/tmp/attachment-drop-capacity-original-${engine}.png`,
            })

            await page.evaluate(() => window.__attachmentProbe.showReplacementOwner())
            assert.deepEqual((await snapshot()).draft, [])
            await page.evaluate(() =>
              window.__attachmentProbe.began?.("capacity-after-free", "dropped"),
            )
            await deliver("capacity-after-free", { files: [dropFile("after-free.pdf")] })
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some(
                  (part) =>
                    part.type === "file" &&
                    part.resource.path === "/verification/after-free.pdf",
                ),
            )
            await page
              .getByRole("button", { name: "Open after-free.pdf", exact: true })
              .waitFor()
            const replacement = await snapshot()
            assert.equal(
              replacement.draft.filter(
                (part) =>
                  part.type === "file" &&
                  part.resource.path === "/verification/after-free.pdf",
              ).length,
              1,
            )
            assert.equal(
              replacement.draft.some(
                (part) =>
                  part.type === "file" &&
                  part.resource.path === "/verification/oldest.pdf",
              ),
              false,
            )
            assert.equal(
              await page.getByText("Too many selections", { exact: true }).count(),
              0,
            )
            await page.screenshot({
              path: `/tmp/attachment-drop-capacity-after-free-${engine}.png`,
            })
          } else if (scenario === "conversation-refusals") {
            const firstId = (await snapshot()).tabs[0].id
            await page.evaluate(() => window.__attachmentProbe.beginDrop("refusal-a"))
            const secondId = await page.evaluate(() =>
              window.__attachmentProbe.openAnotherConversation(),
            )
            assert.ok(secondId && secondId !== firstId)
            await page.evaluate(() => window.__attachmentProbe.beginDrop("refusal-b"))
            await page.evaluate(() =>
              window.__attachmentProbe.finishDropRefusal("refusal-b", "folder-empty"),
            )
            await page.getByText("Folder has no files", { exact: true }).waitFor()
            await page.evaluate(() =>
              window.__attachmentProbe.finishDropRefusal("refusal-a", "folder-too-large"),
            )
            assert.equal(
              await page.getByText("Folder has no files", { exact: true }).count(),
              1,
            )
            assert.equal(
              await page.getByText("Folder is too big to read", { exact: true }).count(),
              0,
            )
            await page.screenshot({ path: `/tmp/attachment-refusal-b-${engine}.png` })

            await page.evaluate(
              (id) => window.__attachmentProbe.selectConversation(id),
              firstId,
            )
            await page.getByText("Folder is too big to read", { exact: true }).waitFor()
            assert.equal(
              await page.getByText("Folder has no files", { exact: true }).count(),
              0,
            )
            await page.screenshot({ path: `/tmp/attachment-refusal-a-${engine}.png` })
            await page.evaluate(() => window.__attachmentProbe.beginDrop("answer-a"))
            await page.evaluate(() =>
              window.__attachmentProbe.drop({
                batch: "answer-a",
                files: [
                  {
                    path: "/verification/answer.pdf",
                    name: "answer.pdf",
                    size: 1,
                    mimeType: "application/pdf",
                    ticket: "answer-a",
                  },
                ],
                refused: null,
                text: { plain: "", uriList: "", html: "" },
              }),
            )
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some((part) => part.type === "file"),
            )
            assert.equal(
              await page.getByText("Folder is too big to read", { exact: true }).count(),
              0,
            )
            await page.evaluate(
              (id) => window.__attachmentProbe.selectConversation(id),
              secondId,
            )
            await page.getByText("Folder has no files", { exact: true }).waitFor()
            assert.equal(
              await page.getByText("Folder has no files", { exact: true }).count(),
              1,
            )
          } else if (scenario === "pathless-host-drop") {
            await page.evaluate(() =>
              window.__attachmentProbe.drop({
                batch: "no-begin-text",
                files: [],
                refused: null,
                text: { plain: "dropped prose", uriList: "", html: "" },
              }),
            )
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some(
                  (part) => part.type === "pasted-text" && part.text === "dropped prose",
                ),
            )
            await page.evaluate(() =>
              window.__attachmentProbe.drop({
                batch: "no-begin-image",
                files: [],
                refused: null,
                text: {
                  plain: "",
                  uriList: "https://attachment-verification.invalid/pathless.png",
                  html: '<img src="https://attachment-verification.invalid/pathless.png">',
                },
              }),
            )
            await page.waitForFunction(
              () => window.__attachmentProbe.snapshot().fetchCalls === 1,
            )
            await page.evaluate(() => window.__attachmentProbe.swapPickerOwner())
            await page
              .locator('[data-attachment-owner="replacement"]')
              .waitFor({ state: "attached" })
            assert.deepEqual((await snapshot()).draft, [])
            await page.evaluate(() => window.__attachmentProbe.completePathlessImage())
            await page.evaluate(() => window.__attachmentProbe.showOriginalOwner())
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some(
                  (part) => part.type === "file" && part.resource.name === "pathless.png",
                ),
            )
            assert.equal(
              (await snapshot()).draft.filter(
                (part) => part.type === "file" && part.resource.name === "pathless.png",
              ).length,
              1,
            )
            await page.screenshot({
              path: resolve(
                root,
                `verification/desktop/evidence/attachment-gesture-owners/pathless-drop-${engine}.png`,
              ),
            })
            await page.evaluate(() => window.__attachmentProbe.showReplacementOwner())
            assert.deepEqual((await snapshot()).draft, [])
          } else if (scenario === "browser-picker-fallback") {
            await page.evaluate(() => (window.__attachmentProbe.browserFallback = true))
            const firstChooser = page.waitForEvent("filechooser")
            await page.getByRole(...selectors.addAttachment).click()
            await page.getByRole(...selectors.addFiles).click()
            await firstChooser
            await page.evaluate(() =>
              document
                .querySelector('input[type="file"]')
                ?.dispatchEvent(new Event("cancel")),
            )

            const secondChooser = page.waitForEvent("filechooser")
            await page.getByRole(...selectors.addAttachment).click()
            await page.getByRole(...selectors.addFiles).click()
            const chooser = await secondChooser
            await page.evaluate(() => window.__attachmentProbe.swapPickerOwner())
            await page
              .locator('[data-attachment-owner="replacement"]')
              .waitFor({ state: "attached" })
            await chooser.setFiles({
              name: "fallback.png",
              mimeType: "image/png",
              buffer: Buffer.from(
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/l9sAAAAASUVORK5CYII=",
                "base64",
              ),
            })
            await page.evaluate(() => window.__attachmentProbe.showOriginalOwner())
            await page.waitForFunction(() =>
              window.__attachmentProbe
                .snapshot()
                .draft.some(
                  (part) => part.type === "file" && part.resource.name === "fallback.png",
                ),
            )
            assert.equal(
              (await snapshot()).draft.filter(
                (part) => part.type === "file" && part.resource.name === "fallback.png",
              ).length,
              1,
            )
            await page.screenshot({
              path: resolve(
                root,
                `verification/desktop/evidence/attachment-gesture-owners/browser-picker-${engine}.png`,
              ),
            })
            await page.evaluate(() => window.__attachmentProbe.showReplacementOwner())
            assert.deepEqual((await snapshot()).draft, [])
          } else {
            for (const name of ["first", "second"]) {
              const batch = `browser-${name}`
              await page.evaluate(
                ({ name, batch }) => {
                  window.__attachmentProbe.began?.(batch, "dropped")
                  window.__attachmentProbe.drop({
                    batch,
                    files: [],
                    refused: null,
                    text: {
                      plain: "",
                      uriList: `https://attachment-verification.invalid/${name}.png`,
                      html: `<img src="https://attachment-verification.invalid/${name}.png">`,
                    },
                  })
                },
                { name, batch },
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
