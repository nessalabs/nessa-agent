#!/usr/bin/env node
/**
 * Opt-in measurement of desktop UI journeys on a seeded in-memory workspace
 * (#595). Not part of run-all. Not a gateway test: the page must not ask for
 * the gateway's conversations (`conversation.list` or
 * `conversation.subscribeList`).
 *
 * Pass is the rendered count equalling the generated count for that surface,
 * and the journeys completing. A Chromium frame over the 50 ms budget is
 * recorded from the calibrated 4× throttle. It does not fail this check.
 * WebKit runs the journeys without that throttle. `performance.memory` is
 * recorded only where the browser exposes it.
 */
import { execSync } from "node:child_process"
import { mkdirSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { attempt, CannotRun, chosen, log, table } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import {
  budgetMs,
  calibrate,
  calibrationFrame,
  exceedsFrameBudget,
  measure,
  observers,
  throttle,
} from "./lib/perf.mjs"
import { main } from "./lib/run.mjs"
import { content, css, keys, names, zoneSaid } from "./lib/selectors.mjs"
import { seededLoadSpec, seededSearch, withSeeded } from "./lib/seeded-load.mjs"
import {
  contentIs,
  focusComposer,
  lift,
  openPanes,
  order,
  paneCount,
  paneDropFailure,
  recordZones,
  settled,
  zoneSays,
} from "./lib/workspace.mjs"

/** A request for the gateway's conversations, one-shot or followed. */
const listsConversations = /conversation\.(list|subscribeList)\b/

const here = dirname(fileURLToPath(import.meta.url))
const evidenceDir = join(here, "../evidence/workspace-load")
const journeys = [
  "identity",
  "list-count",
  "list-search",
  "list-scroll",
  "sidebar-show-all",
  "open",
  "type",
  "overview",
  "overview-scroll",
  "split",
  "drag",
  "close-reopen",
  "repeat-navigation",
]
const countWait = 120_000
const nothing = "zzzz-not-a-session"

const meta = {
  name: "workspace-load",
  summary: "seeded workspace journeys; rendered count equals the generated count",
  defaults: { mode: "prod", engine: "chromium,webkit" },
  options: {
    sessions: { type: "string" },
    seed: { type: "string" },
    now: { type: "string" },
    longTranscripts: { type: "string" },
    messages: { type: "string" },
    messageCharacters: { type: "string" },
    only: { type: "string" },
  },
  help: `
Usage: node verification/desktop/scripts/workspace-load.mjs [options]

Not part of run-all. Defaults to the dry run (seed 590, 10000 sessions, one
long transcript of 8 messages, 4000 ASCII characters) and --mode prod.
--sessions and the other spec flags replace that dry run for a shorter look.
They do not lower the pass condition: the rendered count must equal the
count the page generated for the query it was given.

Chromium records split and drag frames at 4× after calibration (${budgetMs} ms
budget, lib/perf.mjs). A frame over that budget is a finding on the drag
and split rows, not a failure. WebKit does not throttle.

Pass surfaces: the Agents overview after Show All, the columns session list
for the channel that list is showing, and the sidebar branch after Show all
on the channel that branch marks current. The collapsed branch is recorded
and must be short of that channel.`,
}

function asked(options) {
  const spec = { ...seededLoadSpec }
  for (const key of Object.keys(spec)) {
    if (!options.given(key)) continue
    spec[key] = options[key]
  }
  return spec
}

function readSurface(page, channelId) {
  return page.evaluate((id) => {
    const root = document.documentElement
    const raw = root.dataset.seededReport
    const memory = performance.memory
    return {
      report: raw ? JSON.parse(raw) : null,
      uiRevision: root.dataset.uiRevision ?? null,
      refusal: root.dataset.seededRefusal ?? null,
      overview: document.querySelectorAll("[data-overview-item]").length,
      list: document.querySelectorAll(".workspace-list [data-session-row]").length,
      listLabel:
        document.querySelector(".workspace-list")?.getAttribute("aria-label") ?? null,
      branch: id
        ? document.querySelectorAll(`[data-session-row][data-parent="${CSS.escape(id)}"]`)
            .length
        : null,
      dom: document.getElementsByTagName("*").length,
      heap: memory ? memory.usedJSHeapSize : null,
    }
  }, channelId)
}

function channelCounted(report, id) {
  const sessions = report?.channelSessions
  if (typeof id !== "string" || !/^load-[a-z0-9-]+$/.test(id)) return null
  if (!sessions || !Object.hasOwn(sessions, id)) return null
  return { id, count: sessions[id] }
}

/** The channel whose stored name is the list's label. The window chose it. */
function channelNamed(report, label) {
  const names = report?.channelNames
  if (!names || typeof label !== "string" || label === "") return null
  for (const id of Object.keys(names)) {
    if (!Object.hasOwn(names, id) || names[id] !== label) continue
    return channelCounted(report, id)
  }
  return null
}

/** The channel the window is showing, read from that surface, then counted from the report. */
async function shownChannel(page, report, layout) {
  try {
    if (layout === "columns") {
      const list = page.locator(css.sessionList)
      await list.waitFor({ state: "attached", timeout: countWait })
      return channelNamed(report, (await list.getAttribute("aria-label")) ?? "")
    }
    const current = page.locator('[data-row="channel"][data-current]')
    await current.first().waitFor({ state: "attached", timeout: countWait })
    return channelCounted(report, await current.first().getAttribute("data-channel"))
  } catch (error) {
    if (error instanceof Error && error.name === "TimeoutError") return null
    throw error
  }
}

async function dragAcross(page) {
  const grid = await page.locator(css.paneGrid).first().boundingBox()
  if (!grid) throw new CannotRun(`no grid (${css.paneGrid})`)
  await lift(page, 0)
  let announced = false
  for (const [fx, fy] of [
    [0.9, 0.25],
    [0.75, 0.75],
    [0.25, 0.9],
    [0.1, 0.6],
    [0.6, 0.1],
    [0.95, 0.5],
  ]) {
    await page.mouse.move(grid.x + grid.width * fx, grid.y + grid.height * fy, {
      steps: 12,
    })
    if (await zoneSays(page, zoneSaid.any, 2_000)) announced = true
  }
  if (!announced) throw new Error("the drag announced no drop zone")
  await page.mouse.up()
}

async function throttled(context, page, act, settle) {
  const cdp = await throttle(context, page, 4)
  try {
    await page.waitForTimeout(400)
    return await measure(page, act, settle)
  } finally {
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 })
  }
}

const paneOrder = async (page) => (await order(page)).join(",")

await main(meta, async ({ options, rep, url }) => {
  const spec = asked(options)
  const pageUrl = withSeeded(url, spec)
  const commit = execSync("git rev-parse HEAD", { encoding: "utf8" }).trim()
  const picked = options.only ? chosen(options.only, journeys, options.list) : null
  const want = (name) => !picked || picked.includes(name)
  const recordShots =
    options.given("shots") || seededSearch(spec) === seededSearch(seededLoadSpec)
  const shots = recordShots ? (options.shots ?? evidenceDir) : null
  if (shots) mkdirSync(shots, { recursive: true })
  let shotOverview = false
  let shotChannel = false

  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts) {
      const base = { engine, layout }
      const listed = []
      let opened
      const started = Date.now()
      const identity = await attempt(rep, { ...base, name: "identity" }, async () => {
        opened = await openPage(browser, {
          url: pageUrl,
          layout,
          width: 1600,
          height: 1000,
          readyTimeout: 180_000,
          initScripts: engine === "chromium" ? [observers] : [],
          beforeLoad(context) {
            context.on("request", (request) => {
              const body = request.postData() ?? ""
              if (listsConversations.test(request.url()) || listsConversations.test(body))
                listed.push("request")
            })
            context.on("websocket", (socket) => {
              socket.on("framesent", (frame) => {
                if (listsConversations.test(String(frame.payload ?? "")))
                  listed.push("websocket")
              })
            })
          },
        })
        const loadMs = Date.now() - started
        const surface = await readSurface(opened.page, null)
        const report = surface.report
        const failures = []
        if (surface.refusal) failures.push(`seeded page refused ${surface.refusal}`)
        if (!report) failures.push("the page published no seeded report")
        else {
          if (report.generator !== "seeded-workspace")
            failures.push(`generator ${report.generator}`)
          if (String(report.seed) !== String(spec.seed))
            failures.push(`seed ${report.seed}, asked ${spec.seed}`)
          if (String(report.sessions) !== String(spec.sessions))
            failures.push(`sessions ${report.sessions}, asked ${spec.sessions}`)
          if (!surface.uiRevision) failures.push("no UI revision")
        }
        if (listed.length > 0)
          failures.push(`the gateway's conversations were asked for (${listed.length})`)
        for (const error of opened.errors) failures.push(error)
        return {
          failures,
          loadMs,
          commit,
          node: process.version,
          browser: browser.version(),
          uiRevision: surface.uiRevision,
          report,
          listCalls: listed.length,
        }
      })
      if (!identity.ok || !opened) {
        await opened?.close()
        continue
      }
      const { page, context } = opened
      const report = identity.report
      const channel =
        want("list-count") ||
        want("list-search") ||
        want("list-scroll") ||
        want("sidebar-show-all") ||
        want("open") ||
        want("close-reopen") ||
        want("repeat-navigation") ||
        want("overview") ||
        want("overview-scroll")
          ? await shownChannel(page, report, layout)
          : null

      if (layout === "columns" && want("list-count")) {
        await attempt(rep, { ...base, name: "list-count" }, async () => {
          const failures = []
          if (!channel)
            failures.push("the visible session list names no generated channel")
          else {
            await page.waitForFunction(
              ([selector, count]) => document.querySelectorAll(selector).length === count,
              [css.sessionListRow, channel.count],
              { timeout: countWait },
            )
            const surface = await readSurface(page, channel.id)
            if (surface.list !== channel.count)
              failures.push(`session list ${surface.list}, generated ${channel.count}`)
          }
          return {
            failures,
            channel: channel?.id ?? null,
            generated: channel?.count ?? null,
          }
        })
      }

      if (layout === "columns" && want("list-search") && channel) {
        await attempt(rep, { ...base, name: "list-search" }, async () => {
          const field = page.locator(`${css.listSearch} input`)
          await field.fill(nothing)
          await page.waitForFunction(
            (selector) => document.querySelectorAll(selector).length === 0,
            css.sessionListRow,
            { timeout: countWait },
          )
          await field.fill("")
          await page.waitForFunction(
            ([selector, count]) => document.querySelectorAll(selector).length === count,
            [css.sessionListRow, channel.count],
            { timeout: countWait },
          )
          return { failures: [] }
        })
      }

      if (layout === "columns" && want("list-scroll") && channel) {
        await attempt(rep, { ...base, name: "list-scroll" }, async () => {
          await page.locator(css.listScroll).evaluate((element) => {
            element.scrollTop = element.scrollHeight
          })
          const surface = await readSurface(page, channel.id)
          const failures = []
          if (surface.list !== channel.count)
            failures.push(
              `after scroll the list has ${surface.list}, generated ${channel.count}`,
            )
          return { failures }
        })
      }

      if (layout === "sidebar" && want("sidebar-show-all")) {
        await attempt(rep, { ...base, name: "sidebar-show-all" }, async () => {
          const failures = []
          if (!channel) {
            failures.push("the sidebar marks no generated channel current")
            return { failures, collapsed: null, channel: null, generated: null }
          }
          const more = page.locator(`[data-row="more"][data-parent="${channel.id}"]`)
          const text = (await more.count()) ? (await more.first().innerText()).trim() : ""
          const collapsed = (await readSurface(page, channel.id)).branch
          if (names.showAll.test(text)) {
            if (!(collapsed < channel.count))
              failures.push(
                `collapsed branch rendered ${collapsed} of ${channel.count}; it is not the full channel`,
              )
            else await more.first().click()
          } else if (collapsed !== channel.count)
            failures.push(
              `sidebar showed ${collapsed} of ${channel.count} and offered no Show all`,
            )
          if (failures.length === 0) {
            await page.waitForFunction(
              ([id, count]) =>
                document.querySelectorAll(
                  `[data-session-row][data-parent="${CSS.escape(id)}"]`,
                ).length === count,
              [channel.id, channel.count],
              { timeout: countWait },
            )
            const shown = (await readSurface(page, channel.id)).branch
            if (shown !== channel.count)
              failures.push(`sidebar showed ${shown}, generated ${channel.count}`)
          }
          if (shots && engine === "chromium" && !shotChannel) {
            const head = page.locator(
              `[data-row="channel"][data-channel="${channel.id}"]`,
            )
            if (await head.count()) {
              await head.first().screenshot({ path: join(shots, "sidebar-channel.png") })
              shotChannel = true
            }
          }
          return { failures, collapsed, channel: channel.id, generated: channel.count }
        })
      }

      if (want("open") && channel) {
        await attempt(rep, { ...base, name: "open" }, async () => {
          const row =
            layout === "columns"
              ? page.locator(css.sessionListRow).nth(1)
              : page.locator(`[data-session-row][data-parent="${channel.id}"]`).nth(1)
          await row.click()
          await settled(page, 15_000)
          const title = await page.locator(css.conversationTitle).first().textContent()
          const failures = []
          if (!title) failures.push("opening a row did not show a conversation title")
          return { failures }
        })
      }

      if (want("type")) {
        await attempt(rep, { ...base, name: "type" }, async () => {
          await focusComposer(page)
          await page.keyboard.type("seeded load note", { delay: 10 })
          await page.keyboard.press(keys.enter)
          await page.waitForFunction(
            (selector) =>
              document
                .querySelector(selector)
                ?.textContent?.includes("seeded load note") === true,
            css.transcript,
            { timeout: 30_000 },
          )
          return { failures: [] }
        })
      }

      if (want("split")) {
        await attempt(rep, { ...base, name: "split" }, async () => {
          const before = await paneCount(page)
          let frames = null
          const act = () => page.keyboard.press(keys.newSessionBeside)
          if (engine === "chromium") frames = await throttled(context, page, act, 800)
          else await act()
          await page.waitForFunction(
            ([selector, ghost, count]) =>
              [...document.querySelectorAll(selector)].filter(
                (element) => !element.closest(ghost),
              ).length >= count,
            [css.pane, css.dragGhost, before + 1],
            { timeout: 30_000 },
          )
          const after = await paneCount(page)
          const failures = []
          if (after !== before + 1)
            failures.push(`split left ${after} panes, started from ${before}`)
          return {
            failures,
            frames,
            overBudget: frames ? exceedsFrameBudget(frames.maxFrame) : null,
          }
        })
      }

      if (want("drag")) {
        await attempt(rep, { ...base, name: "drag" }, async () => {
          await openPanes(page, 4)
          const zones = await recordZones(page)
          const orderBefore = await paneOrder(page)
          let frames = null
          if (engine === "chromium") {
            const ratio = await calibrate(context, page, 4)
            const frame = await calibrationFrame(page)
            log(
              `calibration [${layout}]: ratio ${ratio.ratio} at 4×, ${frame.cost} ms frame measured ${Math.round(frame.measuredMs)} ms`,
            )
            if (!ratio.ok || !frame.ok)
              return {
                failures: [
                  `calibration did not hold (ratio ${ratio.ratio}, frame ${Math.round(frame.measuredMs)} ms)`,
                ],
              }
            frames = await throttled(context, page, () => dragAcross(page), 1200)
          } else await dragAcross(page)
          await settled(page, 15_000)
          const orderAfter = await paneOrder(page)
          const ghost = await page.locator(css.dragGhost).count()
          const reason = paneDropFailure(
            { order: orderBefore, ghost: 0 },
            { order: orderAfter, ghost },
          )
          const said = await zones.take()
          const dropFailure = reason
            ? `${reason} (${orderBefore} → ${orderAfter}); zones ${JSON.stringify(said)}`
            : null
          if (frames)
            log(
              table(
                [
                  {
                    layout,
                    max: Math.round(frames.maxFrame),
                    over: frames.over,
                    budget: budgetMs,
                  },
                ],
                ["layout", "max", "over", "budget"],
              ),
            )
          return {
            failures: dropFailure ? [dropFailure] : [],
            frames,
            overBudget: frames ? exceedsFrameBudget(frames.maxFrame) : null,
            panes: orderAfter.split(",").filter(Boolean).length,
          }
        })
      }

      if (want("close-reopen") && channel) {
        await attempt(rep, { ...base, name: "close-reopen" }, async () => {
          await page.keyboard.press(keys.closePane)
          await settled(page, 15_000)
          const row =
            layout === "columns"
              ? page.locator(css.sessionListRow).first()
              : page.locator(`[data-session-row][data-parent="${channel.id}"]`).first()
          await row.click()
          await settled(page, 15_000)
          const failures = []
          if ((await paneCount(page)) < 1) failures.push("reopen left no pane")
          if ((await page.locator(css.conversationTitle).count()) < 1)
            failures.push("reopen did not show the conversation")
          return { failures }
        })
      }

      if (want("repeat-navigation")) {
        await attempt(rep, { ...base, name: "repeat-navigation" }, async () => {
          const samples = []
          for (let pass = 0; pass < 2; pass += 1) {
            await focusComposer(page)
            await page.keyboard.press(keys.overview)
            if (!(await contentIs(page, content.overview, 30_000)))
              return { failures: ["repeat navigation did not open the overview"] }
            await page.keyboard.press(keys.escape)
            if (!(await contentIs(page, content.panes, 30_000)))
              return { failures: ["repeat navigation did not leave the overview"] }
            samples.push(await readSurface(page, channel?.id ?? null))
          }
          return {
            failures: [],
            dom: samples.map((sample) => sample.dom),
            heap: samples.map((sample) => sample.heap),
          }
        })
      }

      if (want("overview") && report) {
        await attempt(rep, { ...base, name: "overview" }, async () => {
          const failures = []
          await focusComposer(page)
          await page.keyboard.press(keys.overview)
          if (!(await contentIs(page, content.overview, 60_000)))
            failures.push("the Agents overview did not open")
          else if (report.statusCounts.idle > 0) {
            const ongoing = (await readSurface(page, channel?.id ?? null)).overview
            if (ongoing >= report.sessions)
              failures.push(
                `ongoing overview already listed ${ongoing} of ${report.sessions}`,
              )
            const show = page.getByRole("button", {
              name: names.overviewShowAll,
              exact: true,
            })
            if ((await show.count()) === 0) failures.push("Show All was not offered")
            else {
              const marked = Date.now()
              await show.click()
              await page.waitForFunction(
                (count) =>
                  document.querySelectorAll("[data-overview-item]").length === count,
                report.sessions,
                { timeout: countWait },
              )
              const shown = (await readSurface(page, channel?.id ?? null)).overview
              if (shown !== report.sessions)
                failures.push(`overview listed ${shown}, generated ${report.sessions}`)
              if (
                shots &&
                engine === "chromium" &&
                !shotOverview &&
                failures.length === 0
              ) {
                await page
                  .locator(css.overviewHeader)
                  .screenshot({ path: join(shots, "overview-header.png") })
                shotOverview = true
              }
              return { failures, showAllMs: Date.now() - marked, shown }
            }
          } else {
            const shown = (await readSurface(page, channel?.id ?? null)).overview
            if (shown !== report.sessions)
              failures.push(`overview listed ${shown}, generated ${report.sessions}`)
          }
          return { failures }
        })
      }

      if (want("overview-scroll") && report) {
        await attempt(rep, { ...base, name: "overview-scroll" }, async () => {
          await page.locator(css.overviewScroll).evaluate((element) => {
            element.scrollTop = element.scrollHeight
          })
          const shown = (await readSurface(page, channel?.id ?? null)).overview
          const failures = []
          if (shown !== report.sessions)
            failures.push(
              `after scroll the overview has ${shown}, generated ${report.sessions}`,
            )
          return { failures }
        })
      }

      await opened.close()
    }
  })
})
