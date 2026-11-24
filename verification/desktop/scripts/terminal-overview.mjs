#!/usr/bin/env node
/**
 * Claude and Codex terminal agents with OSC 7501 program status in the sidebar,
 * Agents overview, inline narrow-mode TerminalPeek, and terminal pane header
 * (PR #1408).
 *
 * The page runs as the desktop app does: `gatewayHost` (`lib/fake-host.mjs`)
 * makes `host.kind` native so `main.tsx` composes `hostGateway`, and
 * Playwright's `routeWebSocket` speaks the product protocol with two terminals
 * (a blocked Claude Code session and a working Codex session) plus live
 * `terminal.output` frames.
 */
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { gatewayHost } from "./lib/fake-host.mjs"
import { main } from "./lib/run.mjs"
import { css, keys } from "./lib/selectors.mjs"

const fakeGateway = "ws://127.0.0.1:7499"
const claudeId = `t_${"1".repeat(32)}`
const codexId = `t_${"2".repeat(32)}`

const grant = (action) => ({
  action,
  resource: { organizationId: "personal", id: "gateway" },
})

const sessionReady = {
  version: 1,
  gatewayId: "gateway",
  principalId: "owner",
  organizationId: "personal",
  membershipId: "membership",
  credentialId: "owner-credential",
  audienceId: "gateway",
  expiresAt: 2_000_000_000,
  grants: [grant("conversation.read"), grant("credential.manage")],
  methods: [
    "server.health",
    "channel.list",
    "conversation.subscribeList",
    "conversation.unsubscribe",
    "terminal.list",
    "terminal.attach",
    "terminal.focus",
    "terminal.fit",
    "terminal.detach",
    "terminal.input",
  ],
}

const terminals = [
  {
    terminalId: claudeId,
    title: "Claude",
    kind: "shell",
    requestId: "req-claude",
    folder: "/repo",
    shell: "/bin/zsh",
    runsOn: "here",
    startedAtMs: 1_700_000_000_000,
    state: "running",
    programStatus: {
      state: "blocked",
      kind: "permission",
      app: "claude-code",
      msg: "Allow running cargo test?",
      updatedAtMs: 1_700_000_200_000,
    },
  },
  {
    terminalId: codexId,
    title: "Codex",
    kind: "shell",
    requestId: "req-codex",
    folder: "/repo",
    shell: "/bin/zsh",
    runsOn: "here",
    startedAtMs: 1_700_000_000_000,
    state: "running",
    programStatus: {
      state: "working",
      progress: 40,
      app: "codex",
      msg: "Editing terminal-icon.tsx",
      updatedAtMs: 1_700_000_100_000,
    },
  },
]

function terminalGateway(seen) {
  function route(socket) {
    const nonce = `term-overview-${Math.random().toString(16).slice(2)}`
    let seq = 1
    let subscriptions = 0
    socket.send(
      JSON.stringify({
        type: "event",
        event: "session.challenge",
        seq: 1,
        stateVersion: 0,
        payload: { minVersion: 1, maxVersion: 1, nonce, expiresAt: 2_000_000_000 },
      }),
    )
    socket.onMessage((raw) => {
      let frame
      try {
        frame = JSON.parse(String(raw))
      } catch {
        return
      }
      if (!frame || typeof frame.id !== "string") return
      const send = (result) =>
        socket.send(JSON.stringify({ type: "res", id: frame.id, ...result }))
      if (frame.method === "session.authenticate") {
        send(
          frame.params?.nonce === nonce
            ? { ok: true, payload: sessionReady }
            : { ok: false, error: { code: "unauthorized", message: "nonce" } },
        )
        return
      }
      if (frame.method === "server.health") {
        send({ ok: true, payload: { ok: true, runtimeStatus: "ready", uptimeMs: 1 } })
        return
      }
      if (frame.method === "channel.list") {
        send({ ok: true, payload: { channels: [], complete: true } })
        return
      }
      if (frame.method === "conversation.subscribeList") {
        subscriptions += 1
        const subscriptionId = String(subscriptions)
        send({ ok: true, payload: { subscriptionId } })
        seq += 1
        socket.send(
          JSON.stringify({
            type: "event",
            event: "conversation.listed",
            seq,
            stateVersion: 0,
            payload: {
              subscriptionId,
              list: { conversations: [], complete: true },
            },
          }),
        )
        return
      }
      if (frame.method === "conversation.unsubscribe") {
        send({ ok: true, payload: {} })
        return
      }
      if (frame.method === "terminal.list") {
        send({ ok: true, payload: { terminals } })
        return
      }
      if (frame.method === "terminal.attach") {
        const { terminalId, viewId, cols = 80, rows = 24 } = frame.params ?? {}
        send({ ok: true, payload: {} })
        const emit = (message) => {
          seq += 1
          socket.send(
            JSON.stringify({
              type: "event",
              event: "terminal.output",
              seq,
              stateVersion: 0,
              payload: { terminalId, viewId, message: JSON.stringify(message) },
            }),
          )
        }
        emit({
          type: "ready",
          v: 1,
          cols,
          rows,
          palette: {
            foreground: [230, 230, 230],
            background: [18, 18, 22],
            cursor: [230, 230, 230],
            indexed: Array.from({ length: 256 }, () => [180, 180, 180]),
          },
        })
        const prompt =
          terminalId === claudeId
            ? "Allow running cargo test? [y/n]"
            : "codex> working..."
        emit({
          type: "frame",
          v: 1,
          seq: 1,
          cursor: { col: prompt.length, row: 0, visible: true },
          rows: [
            {
              y: 0,
              cells: [...prompt].map((ch) => [ch, 1, 0]),
            },
          ],
          total: rows,
          scrollback: 0,
        })
        return
      }
      if (
        frame.method === "terminal.focus" ||
        frame.method === "terminal.fit" ||
        frame.method === "terminal.detach"
      ) {
        send({ ok: true, payload: {} })
        return
      }
      if (frame.method === "terminal.input") {
        seen.inputs.push({
          terminalId: frame.params?.terminalId,
          data: frame.params?.data,
        })
        send({ ok: true, payload: {} })
        return
      }
      send({ ok: true, payload: {} })
    })
  }
  return (page) => page.routeWebSocket(fakeGateway, route)
}

const meta = {
  name: "terminal-overview",
  summary:
    "Claude/Codex terminal badges, OSC 7501 status, Agents overview peek, inline PTY key isolation, and pane opening",
  defaults: { engine: "chromium,webkit", layout: "columns" },
  help: `
Usage: node verification/desktop/scripts/terminal-overview.mjs [options]

Checks, per engine:
  sidebar-and-overview  Claude/Codex badges and OSC 7501 status appear in the sidebar and Agents overview with live TerminalPeek
  narrow-pty-keys       in narrow (<820px) inline TerminalPeek, ArrowDown/ArrowUp reach terminal.input without stealing focus to another overview row
  open-terminal-pane    pressing Enter on a terminal row opens its terminal pane with the agent badge and OSC 7501 status`,
}

await main(meta, async ({ options, rep, url }) => {
  await withEngines(options, rep, async (engine, browser) => {
    const seen = { inputs: [] }
    const opened = await openPage(browser, {
      url: new URL("/desktop.html", url).href,
      layout: "columns",
      width: 1120,
      height: 800,
      preparePage: terminalGateway(seen),
      initScripts: [[gatewayHost, { endpoint: fakeGateway, credential: "test-token" }]],
    })
    const { page } = opened
    try {
      await attempt(rep, { engine, name: "sidebar-and-overview" }, async () => {
        const failures = []
        await need(page, `[data-terminal-row="${claudeId}"]`, "Claude terminal row")
        const claudeBadge = await page
          .locator(`[data-terminal-row="${claudeId}"] [data-terminal-agent="claude"]`)
          .count()
        const codexBadge = await page
          .locator(`[data-terminal-row="${codexId}"] [data-terminal-agent="codex"]`)
          .count()
        if (claudeBadge < 1) failures.push("Claude terminal row is missing Claude badge")
        if (codexBadge < 1) failures.push("Codex terminal row is missing Codex badge")

        await page.keyboard.press(keys.overview)
        await need(page, css.overviewListed, "Agents overview listed")
        const overviewClaude = page.locator(`[data-overview-item="${claudeId}"]`)
        const overviewCodex = page.locator(`[data-overview-item="${codexId}"]`)
        if ((await overviewClaude.count()) !== 1)
          failures.push("Claude terminal agent not listed in Agents overview")
        if ((await overviewCodex.count()) !== 1)
          failures.push("Codex terminal agent not listed in Agents overview")
        if ((await overviewClaude.getAttribute("data-kind")) !== "needsYou")
          failures.push("Blocked Claude terminal is not in Needs you group")
        if ((await overviewCodex.getAttribute("data-kind")) !== "working")
          failures.push("Working Codex terminal is not in Working group")

        await need(page, ".agents-terminal-peek [data-slot='terminal']", "TerminalPeek")
        return { failures }
      })

      await attempt(rep, { engine, name: "narrow-pty-keys" }, async () => {
        const failures = []
        await page.setViewportSize({ width: 720, height: 800 })
        const claudeRow = page.locator(`[data-overview-item="${claudeId}"]`)
        await claudeRow.click()
        const inlinePeek = page.locator(
          ".agents-inline-peek .agents-terminal-peek-body [data-slot='terminal']",
        )
        await inlinePeek.waitFor({ state: "visible", timeout: 10_000 })
        await inlinePeek.click()
        const before = seen.inputs.length
        await page.keyboard.press("ArrowDown")
        await page.keyboard.press("ArrowUp")
        const focusInsidePeek = await page.evaluate(() => {
          const body = document.querySelector(
            ".agents-inline-peek .agents-terminal-peek-body",
          )
          return Boolean(body && body.contains(document.activeElement))
        })
        if (!focusInsidePeek)
          failures.push("ArrowDown/ArrowUp inside inline TerminalPeek stole focus")
        if (seen.inputs.length <= before)
          failures.push(
            "Arrow keys inside inline TerminalPeek did not reach terminal.input",
          )
        return { failures, inputsSent: seen.inputs.length - before }
      })

      await attempt(rep, { engine, name: "open-terminal-pane" }, async () => {
        const failures = []
        const claudeRow = page.locator(`[data-overview-item="${claudeId}"]`)
        await claudeRow.focus()
        await page.keyboard.press("Enter")
        const pane = page.locator(
          `.workspace-terminal-pane[data-terminal-agent="claude"][data-program-state="blocked"]`,
        )
        await pane.waitFor({ state: "visible", timeout: 10_000 })
        const headerBadge = await pane.locator('[data-terminal-agent="claude"]').count()
        if (headerBadge < 1)
          failures.push("Opened terminal pane header is missing the Claude badge")
        return { failures }
      })
    } finally {
      await opened.close()
    }
  })
})
