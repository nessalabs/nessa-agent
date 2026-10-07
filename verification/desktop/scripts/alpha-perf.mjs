#!/usr/bin/env node
/**
 * Opt-in alpha sample: cold and warm startup, a handful of panes, and one
 * long seeded transcript. Not part of run-all. Not a frame-budget gate —
 * those rows stay in perf-budget.mjs (sample workspace, #588 / #606). Not a
 * 10,000-chat run — that stays workspace-load.mjs (#595). The observe
 * walk is on main (#607). This script does not time it. A browser
 * timing of that catalogue is a follow-up.
 *
 * Chromium records CDP Performance metrics, Long Animation Frames, and long
 * tasks. WebKit records Navigation Timing, paint, and rAF gaps, without
 * CPU throttling or CDP. A frame over 50 ms is stored on the row. It does
 * not fail the row: this script fails when the window never became ready
 * or the interaction did not do what it is named for.
 */
import { execSync } from "node:child_process"
import { mkdirSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { register } from "tsx/esm/api"
import { TEXT_REPLY_SCENARIO } from "../../../scripts/mcp-test-server/scenarios.mjs"
import { launch, openPage, waitUntilSettled, withEngines } from "./lib/browser.mjs"
import { attempt, CannotRun, chosen, log, table } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import { panelCredential, startGatewayStack } from "./lib/gateway-stack.mjs"
import { exceedsFrameBudget, measure, missingFrameSample, observers, throttle } from "./lib/perf.mjs"
import { main } from "./lib/run.mjs"
import { chordDown, css, keys } from "./lib/selectors.mjs"
import { withSeeded } from "./lib/seeded-load.mjs"
import { target } from "./lib/server.mjs"
import { metricsSince, series, summarizeStartup } from "./lib/startup-sample.mjs"
import { openPanes, paneCount, settled } from "./lib/workspace.mjs"

register()
const { paneLimits } =
  await import("../../../src/desktop/split-panes/model/pane-layout.ts")

const here = dirname(fileURLToPath(import.meta.url))
const evidenceDir = join(here, "../evidence/alpha-perf")
const phases = ["startup", "panes", "transcript", "gateway"]
const viewport = { width: 1600, height: 1000 }

/**
 * One long transcript on a small index. The 10,000-session dry run is
 * workspace-load.mjs. These numbers are this script's, so a run can be
 * compared with the next one.
 */
const transcriptSpec = {
  seed: 590,
  now: 1_700_000_000_000,
  sessions: 24,
  longTranscripts: 1,
  messages: 24,
  messageCharacters: 4_000,
}

const meta = {
  name: "alpha-perf",
  summary: "cold and warm startup, pane count, and one long transcript",
  defaults: { mode: "prod", engine: "chromium,webkit", layout: "columns" },
  options: {
    runs: { type: "string", default: "3" },
    only: { type: "string" },
    "with-gateway": { type: "boolean", default: false },
    agent: { type: "string", default: "claude" },
  },
  help: `
Usage: node verification/desktop/scripts/alpha-perf.mjs [options]

Not part of run-all. Defaults to --mode prod and both engines, columns.
Chromium uses CDP. WebKit has no CPU throttle and no CDP metrics.

  --runs <n>       Cold/warm pairs (default 3).
  --only <list>    startup, panes, transcript, gateway.
  --with-gateway   Also time a scripted gateway's empty window (no agent
                   turn, no catalogue). Needs the gateway built. Adds the
                   gateway phase even when --only is omitted.
  --agent          Passed to the gateway stack (default claude). The
                   scripted agent does not call it.

Cold is a fresh context with the HTTP cache disabled (Chromium). Warm is
two reloads later in that context, cache enabled: the first fills the
cache and is not recorded; the second is the warm sample. WebKit cannot
disable the cache; its cold sample is the first navigation and its warm
sample is the second reload. A CDP counter that moved backwards between
the baseline and the sample is omitted.

Startup readyMs is the harness clock from just before navigation until
the pane (or, for the gateway, the workspace after its index) is settled.
Navigation Timing and paint are the document's own clock. Startup is not
CPU-throttled. Neither clock is a budget. The table's median is the same
upper-middle median as the frame budget's. maxFrameMs is the largest of
each run's longest rAF gap, not a median. WebKit leaves cache null.
The transcript phase uses the columns session list.

Panes fill to paneLimits.maxPanes (the layout's own cap) with the
switcher, then one more split is pressed. That split must not add a pane.
Chromium throttles that refusal. A frame over 50 ms is stored on the row.

Frame rows for overview, drag, and split stay in perf-budget.mjs. A
10,000-session index stays in workspace-load.mjs. Passing those frames
or that index is not this script's result.`,
}

function commit() {
  try {
    return execSync("git rev-parse HEAD", { encoding: "utf8" }).trim()
  } catch {
    return null
  }
}

/**
 * What the document recorded. Evaluated in the page with the startup-screen
 * selector; it closes over nothing else.
 */
function readDocumentSample(startupSelector) {
  const nav = performance.getEntriesByType("navigation")[0]
  const navigation = nav
    ? {
        startTime: nav.startTime,
        duration: nav.duration,
        responseEnd: nav.responseEnd,
        domContentLoadedEventEnd: nav.domContentLoadedEventEnd,
        loadEventEnd: nav.loadEventEnd,
        transferSize: nav.transferSize,
        decodedBodySize: nav.decodedBodySize,
        type: nav.type,
      }
    : null
  const memory = performance.memory
    ? {
        usedJSHeapSize: performance.memory.usedJSHeapSize,
        totalJSHeapSize: performance.memory.totalJSHeapSize,
      }
    : null
  const perf = window.__perf
  const asks = window.__fakeHostAskTimes
  return {
    navigation,
    paints: performance.getEntriesByType("paint").map((entry) => ({
      name: entry.name,
      startTime: entry.startTime,
    })),
    memory,
    gaps: perf?.gaps?.slice() ?? [],
    longtasks: perf?.longtasks?.slice() ?? [],
    loaf: (perf?.loaf ?? []).map((entry) => ({ duration: entry.duration })),
    dom: document.getElementsByTagName("*").length,
    endpointAsks: Array.isArray(asks) ? asks.slice() : [],
    startup: document.querySelector(startupSelector) !== null,
  }
}

async function chromiumSession(context, page, cacheDisabled) {
  const cdp = await context.newCDPSession(page)
  await cdp.send("Performance.enable")
  await cdp.send("Network.enable")
  if (cacheDisabled != null) await cdp.send("Network.setCacheDisabled", { cacheDisabled })
  return cdp
}

async function readMetrics(cdp) {
  if (!cdp) return null
  const result = await cdp.send("Performance.getMetrics").catch(() => null)
  return result?.metrics ?? null
}

/** `baseline` is the CDP reading at the start of this load. Counters are the difference. */
async function sampleOf(page, cdp, readyMs, cache, baseline) {
  const raw = await page.evaluate(readDocumentSample, css.startupScreen)
  const after = await readMetrics(cdp)
  const metricTable = after ? metricsSince(baseline, after) : undefined
  return {
    ...summarizeStartup({ ...raw, metricTable, readyMs, cache }),
    startup: raw.startup,
  }
}

/**
 * Counts a capture-phase delivery of the split-beside chord. The event has
 * to match `keys.newSessionBeside` with the other modifiers up, so a key
 * that never arrived is not recorded as a refusal.
 */
async function armSplitChord(page) {
  const expected = chordDown(keys.newSessionBeside)
  await page.evaluate((expected) => {
    window.__alphaSplitChord = 0
    if (window.__alphaSplitListening) return
    window.__alphaSplitListening = true
    window.addEventListener(
      "keydown",
      (event) => {
        if (
          event.code === expected.code &&
          event.metaKey === expected.metaKey &&
          event.shiftKey === expected.shiftKey &&
          event.altKey === expected.altKey &&
          event.ctrlKey === expected.ctrlKey
        )
          window.__alphaSplitChord += 1
      },
      true,
    )
  }, expected)
}

/**
 * Frames around `act`. `measure` throws when the page recorded no rAF gap,
 * which is after `act` has already run. That gap is a missing sample. The
 * caller still checks what `act` did.
 */
async function framesOf(page, act, settle) {
  try {
    return { frames: await measure(page, act, settle) }
  } catch (error) {
    return { frames: null, unmeasured: missingFrameSample(error) }
  }
}

async function shot(page, shots, name) {
  if (!shots) return
  mkdirSync(shots, { recursive: true })
  const surface = page.locator(css.workspace).first()
  const target = (await surface.count()) ? surface : page
  await target
    .screenshot({ path: join(shots, name), type: "jpeg", quality: 60 })
    .catch((error) => log(`screenshot ${name}: ${error.message}`))
}

function startupFailures(sample) {
  const failures = []
  if (!sample.navigation) failures.push("the document published no navigation timing")
  if (sample.readyMs == null) failures.push("startup had no ready time")
  if (sample.startup) failures.push("the startup screen is still up")
  return failures
}

function startupRow(kind, samples) {
  return {
    kind,
    readyMs: series(samples, (sample) => sample.readyMs).median,
    navigationMs: series(samples, (sample) => sample.navigation?.durationMs).median,
    fcpMs: series(samples, (sample) => sample.paints.firstContentfulPaintMs).median,
    heap: series(samples, (sample) => sample.heapUsed).median,
    maxFrameMs: series(samples, (sample) => sample.frames?.maxGapMs).max,
    runs: series(samples, (sample) => sample.readyMs).runs,
  }
}

/**
 * Cold navigation, then a warm reload. `ready` is the selector that means
 * the window is usable. `prepare` runs before the cold navigation.
 */
async function coldAndWarm(
  browser,
  { url, engine, layout, ready, prepare, shots, shotName },
) {
  let started = 0
  let cdp = null
  let baseline = null
  const opened = await openPage(browser, {
    url,
    layout,
    ...viewport,
    readySelector: ready,
    readyTimeout: prepare?.readyTimeout,
    initScripts: [observers, ...(prepare?.initScripts ?? [])],
    beforeLoad: prepare?.beforeLoad,
    preparePage: async (page, context) => {
      if (engine === "chromium") {
        cdp = await chromiumSession(context, page, true)
        baseline = await readMetrics(cdp)
      }
      started = Date.now()
    },
  })
  try {
    const cold = await sampleOf(
      opened.page,
      cdp,
      Date.now() - started,
      engine === "chromium" ? "disabled" : null,
      baseline,
    )
    await shot(opened.page, shots, shotName)
    if (engine === "chromium" && cdp)
      await cdp.send("Network.setCacheDisabled", { cacheDisabled: false })
    await opened.page.reload({ waitUntil: "domcontentloaded" })
    await opened.page.waitForSelector(ready, { timeout: prepare?.readyTimeout ?? 30_000 })
    await waitUntilSettled(opened.page, prepare?.readyTimeout ?? 30_000)
    if (cdp) baseline = await readMetrics(cdp)
    const warmStart = Date.now()
    await opened.page.reload({ waitUntil: "domcontentloaded" })
    await opened.page.waitForSelector(ready, { timeout: prepare?.readyTimeout ?? 30_000 })
    await waitUntilSettled(opened.page, prepare?.readyTimeout ?? 30_000)
    const warm = await sampleOf(
      opened.page,
      cdp,
      Date.now() - warmStart,
      engine === "chromium" ? "enabled" : null,
      baseline,
    )
    return { opened, cold, warm }
  } catch (error) {
    await opened.close()
    throw error
  }
}

async function scrollTranscript(page) {
  return page.evaluate((selector) => {
    const element = document.querySelector(selector)
    if (!element) return { found: false, overflow: false }
    const overflow = element.scrollHeight > element.clientHeight + 4
    element.scrollTop = 0
    element.scrollTop = element.scrollHeight
    return {
      found: true,
      overflow,
      scrollTop: element.scrollTop,
      scrollHeight: element.scrollHeight,
      clientHeight: element.clientHeight,
    }
  }, css.transcript)
}

function resolvePage(options) {
  const asked = options.only ? chosen(options.only, phases, options.list) : null
  if (asked?.length === 1 && asked[0] === "gateway")
    return { url: null, mode: options.mode, close: async () => {} }
  return target(options)
}

await main(
  meta,
  async ({ options, rep, url }) => {
    const runs = Number(options.runs)
    if (!Number.isInteger(runs) || runs < 1) throw new CannotRun(`--runs ${options.runs}`)
    const picked = options.only ? chosen(options.only, phases, options.list) : null
    const want = (name) => {
      if (picked) return picked.includes(name)
      if (name === "gateway") return options["with-gateway"]
      return true
    }
    const shots = options.shots ?? evidenceDir
    const rows = []
    const head = commit()
    log(`alpha-perf ${head ?? "no commit"} node ${process.version} runs ${runs}`)

    if (want("startup") || want("panes") || want("transcript")) {
      if (!url) throw new CannotRun("no page to measure")
      await withEngines(options, rep, async (engine, browser) => {
        log(`browser ${engine} ${browser.version()}`)
        for (const layout of options.layouts) {
          if (want("startup")) {
            const colds = []
            const warms = []
            await attempt(rep, { name: "startup", engine, layout }, async () => {
              let opened
              try {
                for (let run = 0; run < runs; run++) {
                  const pair = await coldAndWarm(browser, {
                    url,
                    engine,
                    layout,
                    ready: css.pane,
                    shots: run === 0 && engine === "chromium" ? shots : null,
                    shotName: `startup-${layout}.jpg`,
                  })
                  opened = pair.opened
                  colds.push(pair.cold)
                  warms.push(pair.warm)
                  if (run === 0 && engine === "chromium")
                    await shot(pair.opened.page, shots, `startup-warm-${layout}.jpg`)
                  await pair.opened.close()
                  opened = null
                }
              } finally {
                await opened?.close()
              }
              rows.push(
                { engine, layout, ...startupRow("cold", colds) },
                { engine, layout, ...startupRow("warm", warms) },
              )
              return {
                failures: [...colds, ...warms].flatMap(startupFailures),
                commit: head,
                node: process.version,
                cold: colds,
                warm: warms,
              }
            })
          }

          if (want("panes")) {
            await attempt(rep, { name: "panes", engine, layout }, async () => {
              const opened = await openPage(browser, {
                url,
                layout,
                ...viewport,
                initScripts: [observers],
              })
              try {
                // A short fill is this row failing, not "could not run".
                try {
                  await openPanes(opened.page, paneLimits.maxPanes)
                } catch (error) {
                  if (!(error instanceof CannotRun)) throw error
                }
                const filled = await paneCount(opened.page)
                let frames = null
                let unmeasured = null
                let cdp = null
                await armSplitChord(opened.page)
                const chordBefore = await opened.page.evaluate(
                  () => window.__alphaSplitChord ?? 0,
                )
                if (engine === "chromium") {
                  cdp = await throttle(opened.context, opened.page, 4)
                  try {
                    await cdp.send("Performance.enable")
                    await opened.page.waitForTimeout(400)
                    const sampled = await framesOf(
                      opened.page,
                      () => opened.page.keyboard.press(keys.newSessionBeside),
                      800,
                    )
                    frames = sampled.frames
                    unmeasured = sampled.unmeasured ?? null
                  } finally {
                    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 })
                  }
                } else {
                  await opened.page.keyboard.press(keys.newSessionBeside)
                  await settled(opened.page)
                }
                const chordAfter = await opened.page.evaluate(
                  () => window.__alphaSplitChord ?? 0,
                )
                const after = await paneCount(opened.page)
                const surface = await sampleOf(opened.page, cdp, null, null)
                if (engine === "chromium")
                  await shot(opened.page, shots, `panes-${layout}.jpg`)
                const failures = []
                if (filled !== paneLimits.maxPanes)
                  failures.push(
                    `opened ${filled} panes, the layout caps at ${paneLimits.maxPanes}`,
                  )
                if (chordAfter === chordBefore)
                  failures.push("the split chord did not reach the page")
                if (after !== filled)
                  failures.push(
                    `a split past the cap left ${after} panes, the cap is ${paneLimits.maxPanes}`,
                  )
                return {
                  failures,
                  panes: after,
                  cap: paneLimits.maxPanes,
                  dom: surface.dom,
                  heapUsed: surface.heapUsed,
                  nodes: surface.nodes,
                  frames,
                  unmeasured,
                  overBudget: frames ? exceedsFrameBudget(frames.maxFrame) : null,
                  note: "split refused at paneLimits.maxPanes; #588/#606 own the budget rows",
                }
              } finally {
                await opened.close()
              }
            })
          }

          if (want("transcript") && layout === "columns") {
            await attempt(rep, { name: "transcript", engine, layout }, async () => {
              const opened = await openPage(browser, {
                url: withSeeded(url, transcriptSpec),
                layout,
                ...viewport,
                readyTimeout: 60_000,
                initScripts: [observers],
              })
              try {
                const report = await opened.page.evaluate(() => {
                  const raw = document.documentElement.dataset.seededReport
                  if (!raw) return null
                  try {
                    return JSON.parse(raw)
                  } catch {
                    return null
                  }
                })
                const field = (name) =>
                  report && Object.hasOwn(report, name) ? report[name] : null
                // Index 0 is the long transcript. The columns list owns the row.
                const openedLong = await opened.page
                  .locator(css.sessionListRow)
                  .evaluateAll((rows) => {
                    const row = rows.find(
                      (element) =>
                        element.getAttribute("data-session-row") === "load-00000",
                    )
                    if (!row) return false
                    row.click()
                    return true
                  })
                if (openedLong) await settled(opened.page, 15_000)
                await opened.page.waitForSelector(css.transcript, { timeout: 30_000 })
                const plain = await opened.page.locator(css.transcript).innerText()
                let frames = null
                let unmeasured = null
                let cdp = null
                let scrolled
                if (engine === "chromium") {
                  cdp = await throttle(opened.context, opened.page, 4)
                  try {
                    await cdp.send("Performance.enable")
                    await opened.page.waitForTimeout(400)
                    const sampled = await framesOf(
                      opened.page,
                      () => scrollTranscript(opened.page),
                      600,
                    )
                    frames = sampled.frames
                    unmeasured = sampled.unmeasured ?? null
                  } finally {
                    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 })
                  }
                  scrolled = await scrollTranscript(opened.page)
                } else {
                  scrolled = await scrollTranscript(opened.page)
                }
                const surface = await sampleOf(opened.page, cdp, null, null)
                if (engine === "chromium")
                  await shot(opened.page, shots, `transcript-${layout}.jpg`)
                const failures = []
                if (!report) failures.push("the page published no seeded report")
                if (field("sessions") !== transcriptSpec.sessions)
                  failures.push(
                    `seeded sessions ${field("sessions")}, asked ${transcriptSpec.sessions}`,
                  )
                if (field("messages") !== transcriptSpec.messages)
                  failures.push(
                    `long transcript messages ${field("messages")}, asked ${transcriptSpec.messages}`,
                  )
                if (field("longPlainTextCharacters") !== transcriptSpec.messageCharacters)
                  failures.push(
                    `long plain part ${field("longPlainTextCharacters")} characters, asked ${transcriptSpec.messageCharacters}`,
                  )
                if (!openedLong)
                  failures.push("the long transcript is not in the session list")
                if ((plain?.length ?? 0) < transcriptSpec.messageCharacters)
                  failures.push("the open transcript is shorter than the long plain part")
                if (!scrolled?.found) failures.push("no transcript to scroll")
                if (scrolled?.found && !scrolled.overflow)
                  failures.push("the long transcript did not overflow its scroller")
                if (scrolled?.overflow && !(scrolled.scrollTop > 0))
                  failures.push("the scroll did not move")
                return {
                  failures,
                  spec: transcriptSpec,
                  scrolled,
                  dom: surface.dom,
                  heapUsed: surface.heapUsed,
                  nodes: surface.nodes,
                  frames,
                  unmeasured,
                  overBudget: frames ? exceedsFrameBudget(frames.maxFrame) : null,
                }
              } finally {
                await opened.close()
              }
            })
          }
        }
      })
    }

    if (want("gateway")) {
      await attempt(
        rep,
        { name: "gateway", engine: "chromium", layout: "columns" },
        async () => {
          const stack = await startGatewayStack(
            { ...options, mode: "prod", verbose: options.verbose },
            "alpha-perf",
            { as: "panel", scenario: TEXT_REPLY_SCENARIO },
          )
          let browser
          try {
            const endpoint = stack.gateway.url.replace(/^http/, "ws")
            const credential = panelCredential(stack.gateway)
            browser = await launch("chromium", options)
            const colds = []
            const warms = []
            const listed = []
            const origin = new URL(stack.url).origin
            for (let run = 0; run < runs; run++) {
              const pair = await coldAndWarm(browser, {
                url: `${origin}/desktop.html`,
                engine: "chromium",
                layout: "columns",
                ready: css.pane,
                shots: run === 0 ? shots : null,
                shotName: "gateway-startup.jpg",
                prepare: {
                  readyTimeout: 60_000,
                  initScripts: [[gatewayHost, { endpoint, credential }]],
                },
              })
              colds.push(pair.cold)
              warms.push(pair.warm)
              listed.push(await pair.opened.page.locator(css.sessionListRow).count())
              if (run === 0) await shot(pair.opened.page, shots, "gateway-warm.jpg")
              await pair.opened.close()
            }
            const failures = [...colds, ...warms].flatMap(startupFailures)
            if ([...colds, ...warms].some((sample) => sample.endpointAskMs == null))
              failures.push("the page did not ask the host for the gateway endpoint")
            if (listed.some((count) => count !== 0))
              failures.push(
                `the gateway window listed sessions (${listed.join(" ")}); none were created`,
              )
            rows.push(
              {
                engine: "chromium",
                layout: "columns",
                ...startupRow("gateway-cold", colds),
              },
              {
                engine: "chromium",
                layout: "columns",
                ...startupRow("gateway-warm", warms),
              },
            )
            return {
              failures,
              gatewayMs: stack.timings.gatewayMs,
              previewMs: stack.timings.devServerMs,
              cold: colds,
              warm: warms,
              note: "scripted gateway, no conversation created; not a catalogue walk (#607 observe is on main; this script does not time it)",
            }
          } finally {
            await browser?.close()
            await stack.close()
          }
        },
      )
    }

    if (rows.length)
      log(
        `\n${table(rows, ["engine", "layout", "kind", "readyMs", "navigationMs", "fcpMs", "heap", "maxFrameMs", "runs"])}\n(startup ms; heap bytes; max frame is the longest rAF gap during that load)\n`,
      )
  },
  resolvePage,
)
