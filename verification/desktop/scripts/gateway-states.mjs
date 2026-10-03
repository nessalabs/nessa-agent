#!/usr/bin/env node
/**
 * The desktop app's window when it cannot read the local gateway (#419):
 * signed out, the host refusing the credential, or no gateway listening. Each
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
 * What it does not show: a gateway that answers — that is the live run of the
 * packaged app, reported on the pull request.
 */
import { openPage, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"

const fakeGateway = "ws://127.0.0.1:7499"
// Nothing listens here and nothing routes it: the connection is refused.
const noGateway = "ws://127.0.0.1:7498"
const unread = "Nessa couldn’t read the local server’s conversations just now."
const signedOut = "This window isn’t signed in to the local server."
const quietMs = 4_000
// By then the five-round wait is over and the poller has connected once more.
const recoveredMs = 8_000

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
 * Installed before the page's own scripts, as Tauri installs its IPC. The
 * endpoint and credential commands answer as the scenario says, and are
 * counted; every other command waits forever, as an unanswered host does.
 */
function fakeHost({ endpoint, credential }) {
  // The host's own sentences (`GatewayReader::ready`, `CredentialRefusal::NotProvisioned`).
  const notReady = "The local server isn't ready yet"
  const notProvisioned =
    "No chat credential has been provisioned yet. Start the local server " +
    "(`just start`, or `just server`), which creates one on first run."
  let callbacks = 0
  const asked = { load_gateway_endpoint: 0, load_surface_credential: 0 }
  window.__fakeHostAsked = asked
  window.__TAURI_INTERNALS__ = {
    transformCallback: () => ++callbacks,
    invoke(command) {
      if (command === "load_gateway_endpoint") {
        asked[command]++
        window.__fakeHostLastAskAt = performance.now()
        return endpoint === null ? Promise.reject(notReady) : Promise.resolve(endpoint)
      }
      if (command === "load_surface_credential") {
        asked[command]++
        return credential === null
          ? Promise.reject(notProvisioned)
          : Promise.resolve(credential)
      }
      return new Promise(() => {})
    },
  }
}

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

const inside = (inner, outer) =>
  inner.left >= outer.left - 0.5 &&
  inner.top >= outer.top - 0.5 &&
  inner.right <= outer.right + 0.5 &&
  inner.bottom <= outer.bottom + 0.5

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
  if (m.asked.load_gateway_endpoint < 1)
    failures.push("the host's endpoint was never asked")
  if (scenario.credentialNeverAsked && m.asked.load_surface_credential > 0)
    failures.push("the credential was asked for while the gateway was not ready")
  return failures
}

await main(
  {
    name: "gateway-states",
    summary:
      "the desktop app's window says why it cannot read the gateway, never the sample",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, url }) => {
    const origin = new URL(url).origin
    await withEngines(options, rep, async (engine, browser) => {
      for (const scenario of scenarios)
        await attempt(rep, { name: scenario.name, engine, width: 1440 }, async () => {
          const opened = await openPage(browser, {
            url: `${origin}/desktop.html`,
            initScripts: [[fakeHost, scenario]],
            // Either answer: the status this check is for, or a listed
            // session — the sample in disguise, which `check` fails.
            readySelector: `${css.workspaceEmpty}, ${css.sessionRow}`,
            beforeLoad: (context) =>
              context.routeWebSocket(`${fakeGateway}/**`, refuseCredential),
          })
          try {
            const { page } = opened
            const first = await measure(page)
            const failures = check(scenario, first)
            if (!first.button) return { failures, measured: { first } }
            // The poller waits out a failed connect (S10): for `quietMs` after
            // the last ask — inside the five-round wait — nothing asks the host.
            let quiet
            let recovered
            if (scenario.cadence) {
              const { count, since } = await page.evaluate(() => ({
                count: window.__fakeHostAsked.load_gateway_endpoint,
                since: performance.now() - window.__fakeHostLastAskAt,
              }))
              await page.waitForTimeout(Math.max(0, quietMs - since))
              quiet = (await measure(page)).asked.load_gateway_endpoint - count
              if (quiet > 0)
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
            // Try Again reads the index again: the status goes while it reads,
            // which no poll does. Then it says the same while nothing changed.
            await page.evaluate((empty) => {
              window.__statusLeft = false
              new MutationObserver(() => {
                if (!document.querySelector(empty)) window.__statusLeft = true
              }).observe(document.body, { childList: true, subtree: true })
            }, css.workspaceEmpty)
            await page.click(css.workspaceEmptyRetry)
            const read = await page
              .waitForFunction(() => window.__statusLeft, null, { timeout: 5_000 })
              .then(
                () => true,
                () => false,
              )
            if (!read) failures.push("Try Again did not read the index again")
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
            return { failures, measured: { first, again, quiet, recovered } }
          } finally {
            await opened.close()
          }
        })
    })
  },
)
