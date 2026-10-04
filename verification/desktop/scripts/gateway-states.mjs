#!/usr/bin/env node
/**
 * The desktop app's window when it cannot read the local gateway (#419):
 * signed out, the host refusing the credential, the gateway not ready yet, or
 * no gateway listening. Each
 * says why in the chat area, offers Try Again, and never shows the sample in
 * its place.
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
 * The poller's wait it checks against is the gateway source's own
 * (`defaultGatewayTiming`), read in the page from the dev server's module; a
 * production build has none to read, so each scenario there is "could not
 * run" — the script needs --mode dev (the default).
 *
 * What it does not show: a gateway that answers — that is
 * `gateway-window.mjs`'s, against a real one.
 */
import { openPage, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import { main } from "./lib/run.mjs"
import { css, modules } from "./lib/selectors.mjs"
import { inside, modelValue } from "./lib/workspace.mjs"

const fakeGateway = "ws://127.0.0.1:7499"
// Nothing listens here and nothing routes it: the connection is refused.
const noGateway = "ws://127.0.0.1:7498"
const unread = "Nessa couldn’t read the local server’s conversations just now."
const signedOut = "This window isn’t signed in to the local server."
// Try Again's ask comes this soon after the click, or it isn't counted.
const retryAskMs = 1_000
// An endpoint ask at most this old is fresh enough to click after
// (T0, comment 5976195060).
const freshAskMs = 1_000
// The click waits until the host has gone unasked this long, so the connect
// that asked has ended (one connect can ask several times, and a click while
// it runs would join it, S6) and Try Again starts its own.
const settledMs = 400

/**
 * The poller's numbers, from the gateway source's own `defaultGatewayTiming`
 * (`pollMs`, `reconnectRounds`), never copies of them:
 *
 * - `pollerWaitMs`, `pollMs × reconnectRounds`: after a failed connect the
 *   poller refuses `reconnectRounds` rounds, `pollMs` apart at least, counted
 *   from the failure, which comes after its ask. So no poller ask comes
 *   sooner than this after the last one.
 * - `quietMs`, a round short of that wait: nothing asks the host within it.
 * - `recoveredMs`, the wait and three rounds more: the poller's next connect
 *   is at most `reconnectRounds + 1` rounds after the failure (S16), and two
 *   rounds are left for the rounds' own time. It has asked exactly once by
 *   then, as the ask after it is a whole wait later still.
 */
function cadenceOf({ pollMs, reconnectRounds }) {
  const pollerWaitMs = pollMs * reconnectRounds
  return {
    pollMs,
    reconnectRounds,
    pollerWaitMs,
    quietMs: pollerWaitMs - pollMs,
    recoveredMs: pollerWaitMs + 3 * pollMs,
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
    says: unread,
  },
  {
    // The refusal a window meets at every launch, until the host's startup
    // of the gateway is ready (`GatewayReader`, H2′/H4′): the endpoint is
    // refused and the credential is never asked for.
    name: "the gateway is not ready yet",
    endpoint: null,
    credential: "fixture-only",
    says: unread,
    credentialNeverAsked: true,
    cadence: true,
  },
  {
    name: "no gateway listening",
    endpoint: noGateway,
    credential: "fixture-only",
    says: unread,
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
    ([empty, text, retry, chat, rows, sample]) => {
      const rect = (element) => {
        const r = element.getBoundingClientRect()
        return { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
      }
      const status = document.querySelector(empty)
      const button = document.querySelector(retry)
      const area = document.querySelector(chat)
      return {
        text: document.querySelector(text)?.textContent ?? null,
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
    ],
  )
}

function check(scenario, m) {
  const failures = []
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
          const opened = await openPage(browser, {
            url: `${origin}/desktop.html`,
            initScripts: [[gatewayHost, scenario]],
            // Either answer: the status this check is for, or a listed
            // session — the sample in disguise, which `check` fails.
            readySelector: `${css.workspaceEmpty}, ${css.sessionRow}`,
            beforeLoad: (context) =>
              context.routeWebSocket(`${fakeGateway}/**`, refuseCredential),
          })
          try {
            const { page } = opened
            const timing = cadenceOf(
              await modelValue(page, modules.gatewaySource, "defaultGatewayTiming"),
            )
            const { pollerWaitMs, quietMs, recoveredMs } = timing
            const first = await measure(page)
            const failures = check(scenario, first)
            if (!first.button) return { failures, measured: { timing, first } }
            // The poller waits out a failed connect (S10): for `quietMs` after
            // the last ask — a round short of the poller's wait — nothing asks
            // the host.
            let quiet
            let recovered
            if (scenario.cadence) {
              const { count, since } = await page.evaluate(() => ({
                count: window.__fakeHostAsked.load_gateway_endpoint,
                since: performance.now() - window.__fakeHostAskTimes.at(-1),
              }))
              await page.waitForTimeout(Math.max(0, quietMs - since))
              quiet = (await measure(page)).asked.load_gateway_endpoint - count
              // Not `> 0`: a count that is missing fails too.
              if (quiet !== 0)
                failures.push(
                  `the window asked the host ${quiet} times within ${quietMs}ms of its last ask, unprompted`,
                )
              // And then it does connect again, unprompted, exactly once (S16).
              await page.waitForTimeout(recoveredMs - quietMs)
              recovered =
                (await measure(page)).asked.load_gateway_endpoint - count - quiet
              if (recovered !== 1)
                failures.push(
                  `the window asked the host ${recovered} times between ${quietMs}ms and ${recoveredMs}ms after its last ask, not once`,
                )
            }
            // Try Again reads the index again — the status goes while it reads,
            // which no poll does — and connects at once though the poller
            // waits (S12). Then it says the same while nothing changed.
            await page.evaluate(
              ([empty, retry]) => {
                window.__statusLeft = false
                new MutationObserver(() => {
                  if (!document.querySelector(empty)) window.__statusLeft = true
                }).observe(document.body, { childList: true, subtree: true })
                // The click's time, taken before React's own handler, which
                // runs from the root.
                window.__retryClickedAt = null
                window.addEventListener(
                  "click",
                  (event) => {
                    if (window.__retryClickedAt === null && event.target.closest?.(retry))
                      window.__retryClickedAt = performance.now()
                  },
                  { capture: true },
                )
              },
              [css.workspaceEmpty, css.workspaceEmptyRetry],
            )
            // T0: the click placed early in a fresh poller wait — after the
            // last endpoint ask if it is at most `freshAskMs` old, else after
            // the next one, once the asks have settled — so T3's bound is
            // far off.
            const last = await page.evaluate(() => ({
              count: window.__fakeHostAskTimes.length,
              age: performance.now() - window.__fakeHostAskTimes.at(-1),
            }))
            const waitedForAsk = last.count > 0 && !(last.age <= freshAskMs)
            if (waitedForAsk) {
              const asked = await page
                .waitForFunction(
                  (count) => window.__fakeHostAskTimes.length > count,
                  last.count,
                  { polling: "raf", timeout: recoveredMs },
                )
                .then(
                  () => true,
                  () => false,
                )
              if (!asked)
                failures.push(
                  `the window did not ask the host again within ${recoveredMs}ms, unprompted`,
                )
            }
            await page.waitForFunction(
              // With no ask at all there is nothing to settle: T4 fails it.
              (quiet) =>
                window.__fakeHostAskTimes.length === 0 ||
                performance.now() - window.__fakeHostAskTimes.at(-1) >= quiet,
              settledMs,
              { polling: "raf", timeout: recoveredMs },
            )
            await page.click(css.workspaceEmptyRetry)
            const read = await page
              .waitForFunction(() => window.__statusLeft, null, { timeout: 5_000 })
              .then(
                () => true,
                () => false,
              )
            if (!read) failures.push("Try Again did not read the index again")
            // Only an ask within `retryAskMs` of the click counts as Try
            // Again's, and only while the poller's wait since the last ask
            // before the click can't explain it (T1–T4, comment 5975998645).
            const retry = await page
              .waitForFunction(
                (windowMs) => {
                  const clickedAt = window.__retryClickedAt
                  if (clickedAt === null) return false
                  const times = window.__fakeHostAskTimes
                  if (
                    !times.some((t) => t >= clickedAt) &&
                    performance.now() <= clickedAt + windowMs
                  )
                    return false
                  return {
                    lastBefore: times.filter((t) => t < clickedAt).at(-1) ?? null,
                    clickedAt,
                    firstAfter: times.find((t) => t >= clickedAt) ?? null,
                  }
                },
                retryAskMs,
                { timeout: 5_000 },
              )
              .then(
                (handle) => handle.jsonValue(),
                () => null,
              )
            const retryTiming = retry && {
              waitedForAsk,
              sinceLastAsk:
                retry.lastBefore === null ? null : retry.clickedAt - retry.lastBefore,
              askAfterClick:
                retry.firstAfter === null ? null : retry.firstAfter - retry.clickedAt,
            }
            // How far T3's bound is from where the click landed.
            if (retryTiming && retryTiming.sinceLastAsk !== null)
              retryTiming.margin = pollerWaitMs - (retryTiming.sinceLastAsk + retryAskMs)
            if (!retry) failures.push("the click on Try Again was never seen")
            else if (retry.lastBefore === null)
              failures.push("no failed connect came before Try Again for it to beat")
            else if (retryTiming.sinceLastAsk + retryAskMs >= pollerWaitMs)
              failures.push(
                `Try Again was clicked ${Math.round(retryTiming.sinceLastAsk)}ms after the last ask, too late in the poller's ${pollerWaitMs}ms wait to tell its ask from the poller's`,
              )
            else if (
              retryTiming.askAfterClick === null ||
              retryTiming.askAfterClick > retryAskMs
            )
              failures.push("Try Again did not connect while the poller waited")
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
              measured: { timing, first, again, quiet, recovered, retry: retryTiming },
            }
          } finally {
            await opened.close()
          }
        })
    })
  },
)
