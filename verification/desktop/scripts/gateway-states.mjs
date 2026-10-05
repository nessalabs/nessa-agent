#!/usr/bin/env node
/**
 * The desktop app's window when it cannot read the local gateway (#419):
 * signed out, the host refusing the credential, the gateway not ready yet, or
 * no gateway listening. Signed out says why in the chat area and offers
 * Try Again. A startup failure covers the window with the calm screen
 * (the line, STARTUP_GATEWAY, Restart and Quit) and never shows the sample.
 *
 * The page runs as the desktop app does: a fake Tauri host installed before
 * the page's scripts (as `load-fallback.mjs` does) makes `host.kind` native,
 * so `main.tsx` composes `hostGateway` — the real endpoint and credential
 * sources over IPC, `connectDevSession`, the gateway source. Only the host's
 * two answers and the gateway's socket are the scenario's: the socket is
 * Playwright's (`routeWebSocket`), which closes the way a gateway refusing a
 * credential does (4001), or the endpoint names a port nobody listens on —
 * where the browser's own line that the connection failed is the one console
 * error expected.
 *
 * Try Again is checked on a clock the script holds (C0–C4, #419 comment
 * 5977020094). Playwright's clock is installed before the page's scripts, so
 * the poller's rounds and a connect's retry backoff run on its timers; it
 * runs in real time until the script pauses it for the click. While it is
 * paused no timer fires, so a host ask after the click is Try Again's own
 * connect, with no timing number to say so. That rests on a premise: Try
 * Again's connect reaches its first host ask with no page timer, as React's
 * scheduler and IPC promises use none. If that stops holding, the paused
 * clock holds Try Again's own connect, and a correct product fails C2; a
 * broken one never passes.
 *
 * The poller's numbers are the gateway source's own (`defaultGatewayTiming`),
 * read in the page from the dev server's module; a production build has none
 * to read, so each scenario there is "could not run" — the script needs
 * --mode dev (the default).
 *
 * What it does not show: a gateway that answers — that is
 * `gateway-window.mjs`'s, against a real one.
 */
import { openPage, withEngines } from "./lib/browser.mjs"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { gatewayHost } from "./lib/fake-host.mjs"
import { main } from "./lib/run.mjs"
import { css, modules } from "./lib/selectors.mjs"
import { inside, modelValue } from "./lib/workspace.mjs"

const fakeGateway = "ws://127.0.0.1:7499"
// Nothing listens here and nothing routes it: the connection is refused.
const noGateway = "ws://127.0.0.1:7498"
const sentences = JSON.parse(
  readFileSync(
    resolve(
      dirname(fileURLToPath(import.meta.url)),
      "../../../src/host/startup-refusals.json",
    ),
    "utf8",
  ),
)
function sentence(key) {
  if (
    !Object.hasOwn(sentences, key) ||
    typeof sentences[key] !== "string" ||
    !sentences[key]
  )
    throw new Error(`missing startup sentence ${key}`)
  return sentences[key]
}
function startupLine() {
  if (typeof sentences.line !== "string" || !sentences.line)
    throw new Error("missing startup line")
  return sentences.line
}
function startupCode(key) {
  const table = sentences.code
  if (!table || typeof table !== "object" || !Object.hasOwn(table, key))
    throw new Error(`missing startup code ${key}`)
  const value = table[key]
  if (typeof value !== "string" || !value) throw new Error(`startup code ${key} is empty`)
  return value
}
const signedOut = "This window isn’t signed in to the local server."
// How long, in real time, the paused page has to show Try Again's ask (C1).
// It only ends the wait: with the clock paused nothing else asks the host.
const pausedAskWaitMs = 3_000

/**
 * Each of `numbers` is a positive finite number, or the check could not run
 * (T6, comment 5976651213): a number renamed or gone in `source` would make
 * NaN bounds, which pass every comparison they are in by failing it.
 */
function positive(source, numbers) {
  for (const [name, value] of Object.entries(numbers))
    if (!(typeof value === "number" && Number.isFinite(value) && value > 0))
      throw new CannotRun(
        `${source}'s ${name} is ${String(value)}, not a positive finite number`,
      )
}

/**
 * The poller's numbers, from the gateway source's own `defaultGatewayTiming`
 * (`pollMs`, `reconnectRounds`), never copies of them:
 *
 * - `pollerWaitMs`, `pollMs × reconnectRounds`: after a failed connect the
 *   poller refuses `reconnectRounds` rounds, `pollMs` apart at least, counted
 *   from the failure, which comes after its ask. So no poller ask comes
 *   sooner than this after the last one.
 * - `quietMs`, a round short of that wait: nothing asks the host within it.
 *   It also bounds the pause (C5, #419 comment 5978179804): refusals come
 *   at least `pollMs` apart, so under `quietMs` after the last ask at least
 *   one refused round is still owed, and the poller still waits. Past it,
 *   the wait may have run out, and a Try Again that connects only when the
 *   poller would could pass. So `unaskedMs` and `pauseLeadMs` must fit under
 *   it, or the check could not run (C5a), and the pause must come under it,
 *   or it fails (C5b).
 * - `recoveredMs`, the wait and three rounds more: the poller's next connect
 *   is at most `reconnectRounds + 1` rounds after the failure (S16), and two
 *   rounds are left for the rounds' own time. It has asked exactly once by
 *   then, as the ask after it is a whole wait later still.
 * - `unaskedMs`, two rounds, the unasked spell: how long the host goes
 *   unasked before the clock pauses (C1), so the last connect's attempts
 *   have ended. That holds while the client's largest retry backoff plus one
 *   attempt stays under two rounds (`resolveConnectRetry`'s defaults, 500 ms
 *   today); if it stops holding, the result is a spurious C2 failure, not a
 *   pass. A connect still in flight would stop on its paused backoff, and
 *   Try Again would join it (S6) and not ask: a correct product failing C2,
 *   not a broken one passing.
 * - `pauseLeadMs`, a tenth of a round: `pauseAt` takes a time no earlier
 *   than the clock's own, which moves on between the script reading it and
 *   the pause. The timers due in that lead fire as the clock pauses, before
 *   the click; an ask they make is caught by the check at the pause.
 */
function cadenceOf({ pollMs, reconnectRounds }) {
  positive("defaultGatewayTiming", { pollMs, reconnectRounds })
  const pollerWaitMs = pollMs * reconnectRounds
  const quietMs = pollerWaitMs - pollMs
  const unaskedMs = 2 * pollMs
  const pauseLeadMs = pollMs / 10
  // C5a: the unasked spell and the lead before the pause would use up the
  // quiet the pause is meant to land in.
  if (quietMs <= unaskedMs + pauseLeadMs)
    throw new CannotRun(
      `the poller's quiet of ${quietMs}ms, a round short of its wait, is not over the ${unaskedMs}ms unasked spell plus the ${pauseLeadMs}ms lead before the pause: the pause could not land while the poller still waits`,
    )
  return {
    pollMs,
    reconnectRounds,
    pollerWaitMs,
    quietMs,
    recoveredMs: pollerWaitMs + 3 * pollMs,
    unaskedMs,
    pauseLeadMs,
  }
}

const scenarios = [
  {
    name: "signed out: the gateway refuses the credential",
    endpoint: fakeGateway,
    credential: "fixture-only",
    says: signedOut,
    // And the poller's cadence while it stays so (S10).
    cadence: true,
  },
  {
    name: "the host refuses the credential",
    endpoint: fakeGateway,
    credential: null,
    calm: "not-provisioned",
  },
  {
    // The refusal a window meets at every launch, until the host's startup
    // of the gateway is ready (`GatewayReader`, H2′/H4′): the endpoint is
    // refused and the credential is never asked for.
    name: "the gateway is not ready yet",
    endpoint: null,
    credential: "fixture-only",
    calm: "not-ready",
    credentialNeverAsked: true,
    cadence: true,
  },
  {
    name: "no gateway listening",
    endpoint: noGateway,
    credential: "fixture-only",
    calm: "not-listening",
  },
]

/**
 * A gateway refusing the credential it is shown, as `product/socket.rs` does:
 * the challenge first, then — for the client's `session.authenticate` — an
 * `unauthorized` failure under its request id (`failure`, whose message is
 * the code), and the socket closed as `authentication_failed` (4001) with
 * its `SessionTermination` as the reason (`close_session`).
 */
function refuseCredential(socket) {
  socket.send(
    JSON.stringify({
      type: "event",
      event: "session.challenge",
      seq: 1,
      stateVersion: 0,
      payload: {
        minVersion: 1,
        maxVersion: 1,
        nonce: "fixture",
        expiresAt: 2_000_000_000,
      },
    }),
  )
  socket.onMessage((raw) => {
    const frame = JSON.parse(String(raw))
    if (frame.method !== "session.authenticate") return
    socket.send(
      JSON.stringify({
        type: "res",
        id: frame.id,
        ok: false,
        error: { code: "unauthorized", message: "unauthorized" },
      }),
    )
    socket.close({
      code: 4001,
      reason: JSON.stringify({ code: "authentication_failed", retryable: false }),
    })
  })
}

async function measure(page) {
  return page.evaluate(
    ([empty, text, retry, chat, rows, sample, screen, line, code, restart, quit]) => {
      const rect = (element) => {
        const r = element.getBoundingClientRect()
        return { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
      }
      const status = document.querySelector(empty)
      const button = document.querySelector(retry)
      const area = document.querySelector(chat)
      const startup = document.querySelector(screen)
      const restartButton = document.querySelector(restart)
      const quitButton = document.querySelector(quit)
      const onTop = (element) => {
        if (!element) return false
        const r = element.getBoundingClientRect()
        return (
          document
            .elementFromPoint((r.left + r.right) / 2, (r.top + r.bottom) / 2)
            ?.closest("button") === element
        )
      }
      const mark = document.querySelector("[data-nessa-startup-mark]")
      return {
        text: document.querySelector(text)?.textContent ?? null,
        line: document.querySelector(line)?.textContent?.trim() ?? null,
        code: document.querySelector(code)?.textContent?.trim() ?? null,
        startup: startup ? rect(startup) : null,
        restart: restartButton ? rect(restartButton) : null,
        quit: quitButton ? rect(quitButton) : null,
        restartOnTop: onTop(restartButton),
        quitOnTop: onTop(quitButton),
        halo: mark ? getComputedStyle(mark).boxShadow : null,
        status: status ? rect(status) : null,
        button: button ? rect(button) : null,
        buttonOnTop: button
          ? (() => {
              const r = button.getBoundingClientRect()
              return (
                document
                  .elementFromPoint((r.left + r.right) / 2, (r.top + r.bottom) / 2)
                  ?.closest("button") === button
              )
            })()
          : false,
        chat: area ? rect(area) : null,
        viewport: { width: innerWidth, height: innerHeight },
        rows: document.querySelectorAll(rows).length,
        sample: document.querySelectorAll(sample).length,
        asked: { ...window.__fakeHostAsked },
      }
    },
    [
      css.workspaceEmpty,
      css.workspaceEmptyText,
      css.workspaceEmptyRetry,
      css.chatArea,
      css.sessionRow,
      css.sampleAccessory,
      css.startupScreen,
      css.startupLine,
      css.startupCode,
      css.startupRestart,
      css.startupQuit,
    ],
  )
}

function check(scenario, m) {
  if (scenario.calm) return checkCalm(scenario, m)
  const failures = []
  if (m.startup) failures.push("the startup screen covers a signed-out window")
  if (m.text !== scenario.says)
    failures.push(`says ${JSON.stringify(m.text)}, not ${JSON.stringify(scenario.says)}`)
  if (!m.status || !m.chat) failures.push("no status in the chat area")
  else {
    if (!inside(m.status, m.chat))
      failures.push(`the status ${JSON.stringify(m.status)} is outside the chat area`)
    const view = { left: 0, top: 0, right: m.viewport.width, bottom: m.viewport.height }
    if (!inside(m.status, view)) failures.push("the status is outside the window")
  }
  if (!m.button) failures.push("no Try Again")
  else {
    const height = m.button.bottom - m.button.top
    if (height < 24) failures.push(`Try Again is ${height}px tall, under 24`)
    if (!m.buttonOnTop) failures.push("something paints over Try Again")
  }
  // Never the sample in disguise: no session rows, no sample plugin.
  if (m.rows !== 0) failures.push(`${m.rows} session rows listed`)
  if (m.sample !== 0) failures.push("the sample plugin is drawn")
  // Counted: a count that is missing fails, never passes.
  if (!(m.asked.load_gateway_endpoint >= 1))
    failures.push("the host's endpoint was never asked")
  if (scenario.credentialNeverAsked && m.asked.load_surface_credential !== 0)
    failures.push(
      `the credential was asked for while the gateway was not ready (${m.asked.load_surface_credential} times)`,
    )
  return failures
}

function checkCalm(scenario, m) {
  const failures = []
  const line = startupLine()
  const code = startupCode(scenario.calm)
  if (m.line !== line)
    failures.push(`says ${JSON.stringify(m.line)}, not ${JSON.stringify(line)}`)
  if (m.code !== code)
    failures.push(`code is ${JSON.stringify(m.code)}, not ${JSON.stringify(code)}`)
  if (m.line && m.line.includes(sentence(scenario.calm)))
    failures.push("the log sentence is on the screen")
  if (!m.startup) failures.push("no startup screen")
  else if (
    m.startup.left > 1 ||
    m.startup.top > 1 ||
    m.startup.right < m.viewport.width - 1 ||
    m.startup.bottom < m.viewport.height - 1
  )
    failures.push(
      `the startup screen ${JSON.stringify(m.startup)} does not cover the window`,
    )
  for (const [name, box, onTop] of [
    ["Restart", m.restart, m.restartOnTop],
    ["Quit", m.quit, m.quitOnTop],
  ]) {
    if (!box) failures.push(`no ${name}`)
    else {
      const height = box.bottom - box.top
      if (height < 24) failures.push(`${name} is ${height}px tall, under 24`)
      if (!onTop) failures.push(`something paints over ${name}`)
    }
  }
  if (m.halo && m.halo !== "none") failures.push(`the mark has a halo (${m.halo})`)
  if (m.rows !== 0) failures.push(`${m.rows} session rows listed`)
  if (m.sample !== 0) failures.push("the sample plugin is drawn")
  if (!(m.asked.load_gateway_endpoint >= 1))
    failures.push("the host's endpoint was never asked")
  if (scenario.credentialNeverAsked && m.asked.load_surface_credential !== 0)
    failures.push(
      `the credential was asked for while the gateway was not ready (${m.asked.load_surface_credential} times)`,
    )
  return failures
}

await main(
  {
    name: "gateway-states",
    summary:
      "the desktop app's window says why it cannot read the gateway, never the sample",
    defaults: { engine: "chromium,webkit" },
    help: `
It reads the poller's wait from the gateway source in the page, so it needs
--mode dev (the default); under --mode prod each scenario could not run.`,
  },
  async ({ options, rep, url }) => {
    const origin = new URL(url).origin
    await withEngines(options, rep, async (engine, browser) => {
      for (const scenario of scenarios)
        await attempt(rep, { name: scenario.name, engine, width: 1440 }, async () => {
          let opened
          try {
            opened = await openPage(browser, {
              url: `${origin}/desktop.html`,
              initScripts: [[gatewayHost, scenario]],
              // Either answer: the status this check is for, or a listed
              // session — the sample in disguise, which `check` fails.
              readySelector: `${css.workspaceEmpty}, ${css.sessionRow}, ${css.startupScreen}`,
              beforeLoad: async (context) => {
                // C0: the page's timers are the clock's, running in real time.
                await context.clock.install()
                await context.routeWebSocket(`${fakeGateway}/**`, refuseCredential)
              },
            })
            const { page, context } = opened
            const timing = cadenceOf(
              await modelValue(page, modules.gatewaySource, "defaultGatewayTiming"),
            )
            const { quietMs, recoveredMs, unaskedMs, pauseLeadMs } = timing
            const first = await measure(page)
            const failures = check(scenario, first)
            if (scenario.calm) {
              // Restart restarts the app. The poller, under the screen, is
              // what asks again. Cadence below still runs when the scenario
              // asks for it; there is no Try Again to click.
            } else if (!first.button) return { failures, measured: { timing, first } }
            // The poller waits out a failed connect (S10): for `quietMs` after
            // the last ask — a round short of the poller's wait — nothing asks
            // the host. The page's `performance.now()` is the clock's, which
            // runs in real time until the pause below.
            let asksInQuiet
            let asksByRecovered
            if (scenario.cadence) {
              const { count, since } = await page.evaluate(() => ({
                count: window.__fakeHostAsked.load_gateway_endpoint,
                since: performance.now() - window.__fakeHostAskTimes.at(-1),
              }))
              await page.waitForTimeout(Math.max(0, quietMs - since))
              asksInQuiet = (await measure(page)).asked.load_gateway_endpoint - count
              // Not `> 0`: a count that is missing fails too.
              if (asksInQuiet !== 0)
                failures.push(
                  `the window asked the host ${asksInQuiet} times within ${quietMs}ms of its last ask, unprompted`,
                )
              // And then it does connect again, unprompted, exactly once (S16).
              await page.waitForTimeout(recoveredMs - quietMs)
              asksByRecovered =
                (await measure(page)).asked.load_gateway_endpoint - count - asksInQuiet
              if (asksByRecovered !== 1)
                failures.push(
                  `the window asked the host ${asksByRecovered} times between ${quietMs}ms and ${recoveredMs}ms after its last ask, not once`,
                )
            }
            if (scenario.calm) {
              const refused = `WebSocket connection to '${noGateway}/session' failed`
              failures.push(
                ...opened.errors.filter(
                  (error) =>
                    !(scenario.endpoint === noGateway && error.includes(refused)),
                ),
              )
              return {
                failures,
                measured: { timing, first, asksInQuiet, asksByRecovered },
              }
            }
            // Try Again reads the index again — the status goes while it reads,
            // which no poll does — and connects at once though the poller
            // waits (S12). Then it says the same while nothing changed.
            await page.evaluate((empty) => {
              window.__statusLeft = false
              new MutationObserver(() => {
                if (!document.querySelector(empty)) window.__statusLeft = true
              }).observe(document.body, { childList: true, subtree: true })
            }, css.workspaceEmpty)
            // C1: once the host has gone unasked for `unaskedMs`, the clock
            // pauses, and no timer fires until it resumes. One evaluate waits
            // for that and answers with the page's time, so only its answer
            // comes between the unasked spell and `pauseAt`. It polls on the
            // page's timers, the clock's, still running in real time.
            const unaskedAt = await page.evaluate(
              ([unasked, timeout]) =>
                new Promise((resolve) => {
                  const started = performance.now()
                  const poll = () => {
                    const last = window.__fakeHostAskTimes.at(-1)
                    if (last === undefined || performance.now() - last >= unasked)
                      resolve(Date.now())
                    else if (performance.now() - started >= timeout) resolve(null)
                    else setTimeout(poll, 10)
                  }
                  poll()
                }),
              [unaskedMs, recoveredMs],
            )
            if (unaskedAt === null) {
              failures.push(
                `the host was never unasked for ${unaskedMs}ms within ${recoveredMs}ms, so the clock was not paused for Try Again`,
              )
              return {
                failures,
                measured: { timing, first, asksInQuiet, asksByRecovered },
              }
            }
            await context.clock.pauseAt(unaskedAt + pauseLeadMs)
            const atPause = await page.evaluate(() => {
              const times = window.__fakeHostAskTimes
              return {
                asked: times.length,
                unaskedMs: times.length === 0 ? null : performance.now() - times.at(-1),
              }
            })
            // C3: with no failed connect before it, there is nothing for Try
            // Again to beat.
            if (atPause.asked === 0)
              failures.push("no failed connect came before Try Again for it to beat")
            else if (!(atPause.unaskedMs >= unaskedMs))
              failures.push(
                `the host was asked ${Math.round(atPause.unaskedMs)}ms before the clock paused, under ${unaskedMs}ms: Try Again may join that connect`,
              )
            // C5b: under `quietMs`, a refused round is still owed, so the
            // poller still waits when Try Again is clicked.
            else if (!(atPause.unaskedMs < quietMs))
              failures.push(
                `the clock paused ${Math.round(atPause.unaskedMs)}ms after the last ask, not under ${quietMs}ms: the poller's wait may have ended, so Try Again's connect is not shown to beat it`,
              )
            // A real click, so Playwright's own checks (visible, stable, not
            // painted over) come first.
            const clicked = await page
              .click(css.workspaceEmptyRetry, { timeout: 5_000 })
              .then(
                () => null,
                (error) => error.message.split("\n")[0],
              )
            if (clicked !== null) {
              failures.push(`Try Again could not be clicked: ${clicked}`)
              return {
                failures,
                measured: { timing, first, asksInQuiet, asksByRecovered, atPause },
              }
            }
            const clickedAt = Date.now()
            // C1 and C2: the clock still paused, an ask after the click is Try
            // Again's own connect. With none, it did not connect, or its
            // connect was held, on a connect in flight or a page timer.
            const asked = await page
              .waitForFunction(
                (count) => window.__fakeHostAskTimes.length > count,
                atPause.asked,
                { timeout: pausedAskWaitMs },
              )
              .then(
                () => true,
                () => false,
              )
            const tryAgain = {
              ...atPause,
              askSeenMs: asked ? Date.now() - clickedAt : null,
            }
            if (!asked)
              failures.push(
                `Try Again did not connect: no host ask within ${pausedAskWaitMs}ms of the click with the clock paused. Either Try Again does not connect, or it joined a connect still in flight (S6), or its connect waits on a page timer, which the paused clock holds`,
              )
            const read = await page
              .waitForFunction(() => window.__statusLeft, null, { timeout: 5_000 })
              .then(
                () => true,
                () => false,
              )
            if (!read) failures.push("Try Again did not read the index again")
            // C4: the clock runs again, and the window settles as before.
            await context.clock.resume()
            await page.waitForSelector(css.workspaceEmpty, { timeout: 10_000 })
            const again = await measure(page)
            failures.push(
              ...check(scenario, again).map((failure) => `after Try Again: ${failure}`),
            )
            const refused = `WebSocket connection to '${noGateway}/session' failed`
            failures.push(
              ...opened.errors.filter(
                (error) => !(scenario.endpoint === noGateway && error.includes(refused)),
              ),
            )
            return {
              failures,
              measured: {
                timing,
                first,
                again,
                asksInQuiet,
                asksByRecovered,
                tryAgain,
              },
            }
          } finally {
            // The body's result or error is the one reported: a close that
            // fails is swallowed, so it cannot take its place.
            await opened?.close().catch(() => {})
          }
        })
    })
  },
)
