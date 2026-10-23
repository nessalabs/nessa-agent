import assert from "node:assert/strict"
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { performance } from "node:perf_hooks"
import { fixtureSite } from "../fixtures/site.mjs"
import { launchBrowser } from "../runtime/browser.mjs"
import { viewerServer } from "../runtime/server.mjs"
import { chromium } from "playwright"
import { adequatelySandboxed } from "./sandbox-status.mjs"
import { closeResources } from "../runtime/cleanup.mjs"
import { finishVerification, failureDetails } from "./finish-verification.mjs"

const output = new URL("../evidence/latest/", import.meta.url)
await mkdir(output, { recursive: true })
const root = await mkdtemp(join(tmpdir(), "nessa-1089-test-"))
let site
const report = {
  timestamp: new Date().toISOString(),
  platform: process.platform,
  node: process.version,
  uid: process.getuid?.(),
  chromiumSandbox: true,
  headless: false,
  tests: [],
  measurements: {},
}
let browser
let viewer
let unconfirmedStartupCleanup
let competingContext
const configuration = (name) => ({
  profile: join(root, name),
  downloads: join(root, "downloads", name),
  home: site.home,
  executablePath: process.env.EXPERIMENT_CHROMIUM,
})
const started = performance.now()
async function check(name, run) {
  await run()
  report.tests.push({ name, result: "passed" })
}
try {
  site = await fixtureSite()
  browser = await launchBrowser(configuration("a"))
  report.measurements.startupMs = performance.now() - started
  report.browserVersion = browser.context.browser()?.version() ?? "unknown"
  await check("sandbox renderer status", async () => {
    const sandbox = await browser.context.newPage()
    await sandbox.goto("chrome://sandbox")
    const text = await sandbox.locator("body").innerText()
    await writeFile(new URL("sandbox.txt", output), text)
    // Linux check is stronger than merely checking launch arguments.
    if (process.platform === "linux") {
      const status = await sandbox.evaluate(() => ({
        rows: [...document.querySelectorAll("#sandbox-status tr")].map((row) =>
          [...row.querySelectorAll("td")].map((cell) => cell.textContent.trim()),
        ),
        evaluation: document.querySelector("#evaluation")?.innerText.trim(),
      }))
      assert.equal(adequatelySandboxed(status.rows, status.evaluation), true)
    }
    await sandbox.close()
  })
  await check(
    "minimal MV3 content script, messaging, service worker, storage",
    async () => {
      await browser.page.waitForFunction(
        () => Number(document.querySelector("#extension").dataset.visits) > 0,
      )
    },
  )
  const visits = Number(
    await browser.page.locator("#extension").getAttribute("data-visits"),
  )
  viewer = await viewerServer(browser.session, browser.savedDownloads)
  const authorization = {
    Authorization: `Bearer ${viewer.token}`,
    "Content-Type": "application/json",
  }
  await check("agent API and viewer pointer control affect the same page", async () => {
    const response = await fetch(viewer.origin + "/command", {
      method: "POST",
      headers: authorization,
      body: JSON.stringify({ type: "increment" }),
    })
    assert.equal(response.status, 200)
    const human = await browser.context.newPage()
    await human.goto(viewer.url)
    await human.waitForFunction(
      () => document.querySelector("#screen").naturalWidth === 1000,
    )
    const button = await browser.page.locator("#counter").boundingBox()
    const image = await human.locator("#screen").boundingBox()
    await human.mouse.click(
      image.x + ((button.x + button.width / 2) * image.width) / 1000,
      image.y + ((button.y + button.height / 2) * image.height) / 700,
    )
    await browser.page.waitForFunction(() => localStorage.getItem("count") === "2")
    await human.screenshot({
      path: new URL("shared-viewer.png", output).pathname,
      fullPage: true,
    })
    await human.close()
    await browser.page.locator("#note").focus()
    await browser.session.run({ type: "text", text: "test-only persistent note" })
    await browser.context.addCookies([
      {
        name: "nessa_test",
        value: "persisted",
        url: site.home,
        expires: Date.now() / 1000 + 86400,
        sameSite: "Strict",
      },
    ])
  })
  await check("navigation, download bytes, popup shares context", async () => {
    await browser.session.run({ type: "navigate", target: "next" })
    assert.equal(new URL(browser.page.url()).pathname, "/next")
    const event = browser.page.waitForEvent("download")
    await browser.page.locator("#download").click()
    const download = await event
    assert.equal(await download.failure(), null)
    const downloadPath = await download.path()
    assert.equal(await readFile(downloadPath, "utf8"), "nessa experiment test download\n")
    const popupEvent = browser.context.waitForEvent("page")
    await browser.page.getByText("Open popup").click()
    const popup = await popupEvent
    await popup.waitForLoadState("domcontentloaded")
    assert.equal(await popup.evaluate(() => localStorage.getItem("count")), "2")
    await popup.close()
  })
  await check("failed navigation recovers; invalid commands are refused", async () => {
    await assert.rejects(
      browser.session.run({ type: "navigate", target: "unavailable" }),
      { code: "browser_operation_failed" },
    )
    await browser.session.run({ type: "navigate", target: "home" })
    await assert.rejects(browser.session.run({ type: "evaluate" }), {
      code: "invalid_command",
    })
  })
  await check("profile rejects competing browser owner", async () => {
    let refused = false
    try {
      competingContext = await chromium.launchPersistentContext(
        configuration("a").profile,
        {
          ...(await import("../runtime/browser.mjs")).launchOptions(
            process.env.EXPERIMENT_CHROMIUM,
          ),
          timeout: 5000,
        },
      )
    } catch {
      refused = true
    } finally {
      await competingContext?.close()
      competingContext = undefined
    }
    assert.equal(refused, true)
  })
  const timings = [],
    sizes = []
  for (let i = 0; i < 20; i++) {
    const at = performance.now()
    const response = await fetch(viewer.origin + "/frame", { headers: authorization })
    assert.equal(response.status, 200)
    sizes.push((await response.arrayBuffer()).byteLength)
    timings.push(performance.now() - at)
  }
  timings.sort((a, b) => a - b)
  report.measurements.frames = {
    samples: 20,
    medianMs: timings[10],
    maxMs: timings[19],
    meanBytes: sizes.reduce((a, b) => a + b) / sizes.length,
  }
  await viewer.close()
  viewer = undefined
  await browser.close()
  browser = undefined
  await check("cookie, localStorage and extension storage survive restart", async () => {
    browser = await launchBrowser(configuration("a"))
    assert.equal(await browser.page.evaluate(() => localStorage.getItem("count")), "2")
    assert.equal(
      await browser.page.locator("#note").inputValue(),
      "test-only persistent note",
    )
    assert.ok(
      (await browser.context.cookies()).some(
        (cookie) => cookie.name === "nessa_test" && cookie.value === "persisted",
      ),
    )
    await browser.page.waitForFunction(
      (previous) =>
        Number(document.querySelector("#extension").dataset.visits) > previous,
      visits,
    )
    await browser.close()
    browser = undefined
  })
  await check("second profile has independent cookies and storage", async () => {
    browser = await launchBrowser(configuration("b"))
    assert.equal(await browser.page.evaluate(() => localStorage.getItem("count")), null)
    assert.equal(await browser.page.locator("#note").inputValue(), "")
    assert.ok(
      !(await browser.context.cookies()).some((cookie) => cookie.name === "nessa_test"),
    )
    await browser.page.waitForFunction(
      () => document.querySelector("#extension").dataset.visits === "1",
    )
  })
  await check("browser disconnect fences commands", async () => {
    await browser.context.close()
    await assert.rejects(browser.session.run({ type: "increment" }), {
      code: "session_closed",
    })
  })
  report.result = "passed"
} catch (error) {
  unconfirmedStartupCleanup = error.cleanupOwner
  report.result = report.tests.length === 0 ? "blocked" : "failed"
  report.measurements.failureAfterMs = performance.now() - started
  report.error = failureDetails(error)
  process.exitCode = report.result === "blocked" ? 2 : 1
} finally {
  await finishVerification({
    report,
    root,
    close: async () => {
      await closeResources([viewer, browser, site])
    },
    hasUnconfirmedResources: () => Boolean(unconfirmedStartupCleanup || competingContext),
    removeProfile: () => rm(root, { recursive: true, force: true }),
    writeReport: (value) =>
      writeFile(
        new URL("browser-results.json", output),
        JSON.stringify(value, null, 2) + "\n",
      ),
  })
  if (report.result === "failed") process.exitCode = 1
  console.log(JSON.stringify(report))
}
