/**
 * Indicative commit-to-screen timings against a scripted gateway.
 *
 * Dev build, Chromium, one warmup and nine trials. The panel is not measured.
 * A message is accepted when conversation.send resolves. A reply is accepted
 * when a read of that conversation first returns its text; the read is issued
 * on a tight loop, and both the start and the return of that read are kept.
 * The screen time is when the open transcript shows it. The session row is
 * not timed: its preview is the last completed reply and its clock stays
 * "now" for a minute, so the row text does not move with the message.
 *
 * Run from a checkout: node verification/desktop/scripts/commit-latency.mjs <label>
 */
import { randomUUID } from "node:crypto"
import { join } from "node:path"
import { pathToFileURL } from "node:url"
import { setTimeout as sleep } from "node:timers/promises"
import { chromium } from "playwright"

const root = process.cwd()
const lib = (name) =>
  pathToFileURL(join(root, "verification/desktop/scripts/lib", name)).href
const { startGatewayStack, panelCredential } = await import(lib("gateway-stack.mjs"))
const { openPage } = await import(lib("browser.mjs"))
const { gatewayHost } = await import(lib("fake-host.mjs"))
const { TEXT_REPLY_SCENARIO } = await import(
  pathToFileURL(join(root, "scripts/mcp-test-server/scenarios.mjs")).href
)

const label = process.argv[2] ?? "run"
const measured = 9
const promptFor = (marker) =>
  `${marker}: reply with exactly that word and nothing else, using no tools.`

function median(values) {
  const sorted = values.filter((value) => Number.isFinite(value)).sort((a, b) => a - b)
  if (sorted.length === 0) return null
  const mid = Math.floor(sorted.length / 2)
  return sorted.length % 2 === 0 ? (sorted[mid - 1] + sorted[mid]) / 2 : sorted[mid]
}

function replyText(message) {
  return (message?.parts ?? [])
    .filter((part) => part.kind === "text")
    .map((part) => part.text)
    .join("")
}

const stack = await startGatewayStack(
  { agent: "claude", mode: "dev" },
  `latency-${label}`,
  {
    as: "panel",
    scenario: TEXT_REPLY_SCENARIO,
  },
)

const pageErrors = []
let browser
try {
  const conversationId = randomUUID()
  const opening = `L${randomUUID().slice(0, 8)}`
  await stack.client.conversation.create({ conversationId, agent: "claude" })
  await stack.client.conversation.send(conversationId, promptFor(opening))
  const opened = Date.now() + 20_000
  let title
  while (Date.now() < opened) {
    const view = await stack.client.conversation.read(conversationId)
    const turn = view.messages.find((message) =>
      String(message.userText ?? "").includes(opening),
    )
    if (turn?.status === "completed" && replyText(turn).includes("Ready.")) break
    await sleep(50)
  }
  const listed = await stack.client.conversation.list({})
  title = listed.conversations.find((row) => row.conversationId === conversationId)?.title
  if (!title) throw new Error("the conversation has no title")

  browser = await chromium.launch({ headless: true, channel: "chrome" })
  const openedPage = await openPage(browser, {
    url: `${new URL(stack.url).origin}/desktop.html`,
    layout: "columns",
    initScripts: [
      [
        gatewayHost,
        {
          endpoint: stack.gateway.url.replace(/^http/, "ws"),
          credential: panelCredential(stack.gateway),
        },
      ],
    ],
  })
  const page = openedPage.page
  page.on("pageerror", (error) => pageErrors.push(String(error)))
  const row = page.locator("[data-drag-item]", { hasText: opening }).first()
  await row.waitFor({ timeout: 30_000 })
  await row.click()
  await page.locator(".workspace-message[data-role]").first().waitFor({ timeout: 20_000 })
  await sleep(500)

  const samples = []
  for (let index = 0; index < measured + 1; index += 1) {
    const marker = `M${index}${randomUUID().slice(0, 6)}`
    const beforeAgents = await page
      .locator('.workspace-message[data-role="agent"]')
      .count()
    let messageScreen = null
    let replyScreen = null
    let replyLo = null
    let replyHi = null
    let stop = false
    const watchDom = (async () => {
      const end = Date.now() + 20_000
      while (!stop && Date.now() < end) {
        const now = Date.now()
        if (messageScreen === null) {
          const users = await page
            .locator('.workspace-message[data-role="user"]')
            .allTextContents()
          if (users.some((text) => text.includes(marker))) messageScreen = now
        }
        if (replyScreen === null) {
          const agents = page.locator('.workspace-message[data-role="agent"]')
          const count = await agents.count()
          if (count > beforeAgents) {
            const text = await agents.nth(count - 1).innerText()
            if (text.includes("Ready.")) replyScreen = now
          }
        }
        if (messageScreen !== null && replyScreen !== null) return
        await sleep(20)
      }
    })()
    const messageAcceptedAt = Date.now()
    await stack.client.conversation.send(conversationId, promptFor(marker))
    const messageAccepted = Date.now()
    const watchRead = (async () => {
      const end = Date.now() + 20_000
      while (replyHi === null && Date.now() < end) {
        const started = Date.now()
        const view = await stack.client.conversation.read(conversationId)
        const returned = Date.now()
        const turn = view.messages.find((message) =>
          String(message.userText ?? "").includes(marker),
        )
        if (replyText(turn).includes("Ready.")) {
          replyLo = started
          replyHi = returned
          return
        }
        await sleep(20)
      }
    })()
    await Promise.all([watchDom, watchRead])
    stop = true
    const sample = {
      warmup: index === 0,
      messageMs: messageScreen === null ? null : messageScreen - messageAccepted,
      replyMs: replyScreen === null || replyHi === null ? null : replyScreen - replyHi,
      replyFromReadStartMs:
        replyScreen === null || replyLo === null ? null : replyScreen - replyLo,
      sendToAcceptMs: messageAccepted - messageAcceptedAt,
    }
    samples.push(sample)
    process.stderr.write(`${label} ${index} ${JSON.stringify(sample)}\n`)
  }
  const kept = samples.filter((sample) => !sample.warmup)
  const report = {
    label,
    root,
    trials: kept.length,
    medians: {
      messageMs: median(kept.map((sample) => sample.messageMs)),
      replyMs: median(kept.map((sample) => sample.replyMs)),
      replyFromReadStartMs: median(kept.map((sample) => sample.replyFromReadStartMs)),
    },
    samples: kept,
    pageErrors,
  }
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`)
  await openedPage.close()
} finally {
  await browser?.close().catch(() => undefined)
  await stack.close()
}
