#!/usr/bin/env node
/**
 * Settings › Connections › Integrations over a real gateway (#391): the
 * gateway's stored MCP servers, managed from the desktop window of a browser
 * preview (`desktop.html?gateway`), every write going through a real gateway
 * to its `config.json`.
 *
 * It starts, each of its own and on 127.0.0.1:
 * - a gateway in a temporary `ci` namespace with one agent's runtime
 *   (`--agent claude|codex`, default claude) and no MCP server stored
 *   (`scripts/mcp-test-server/local-gateway.mjs`, `mcpServers: []`);
 * - a dev server whose `/browser` proxy is that gateway.
 *
 * In each engine it signs a page in with the gateway's owner token and walks
 * the tab: add the test MCP server (`scripts/mcp-test-server/server.mjs`,
 * slow to start, so an inspection is seen running) with one variable,
 * inspect it, follow focus through each part that opens and closes, turn it
 * off, rename it, give it another command (every value entered again),
 * measure it narrow, meet a conflict a Node client makes
 * first, remove it, see a failed list's notice go once a reconnect lists;
 * meet a list too large to show, and a save refused for size; meet two
 * servers stored by hand under one name, neither editable, the one asked
 * from confirmed and the gateway removing the first stored; then a
 * credential that may only converse sends one list, is refused it, and sees
 * the administrator notice and no control. Last, the issue's Done-when: the server added
 * again from the window, a conversation in which the agent calls
 * `show_chart`, and the chart's app drawn once in the window.
 *
 * The page's socket to the gateway is routed through the script
 * (`routeWebSocket`), which counts each mcpServers request sent on it and
 * can drop it, as a lost connection would, for the window to reconnect.
 *
 * The variable's value is generated, never printed, and must appear nowhere
 * in the page once saved. The owner token is the gateway's own, read from its
 * temporary directory and removed with it. The Done-when step uses the agent's
 * sign-in on this machine and creates no account.
 *
 * Every step runs on one page per engine, in order; a step that fails stops
 * those after it, which are reported as not run.
 */
import { randomUUID } from "node:crypto"
import { existsSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { setTimeout as sleep } from "node:timers/promises"

import {
  SERVER,
  agentCommand,
  serverScript,
  startLocalGateway,
} from "../../../scripts/mcp-test-server/local-gateway.mjs"
import { appFrame, oneMount } from "./lib/apps.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { CannotRun, chosen, log } from "./lib/cli.mjs"
import { admitOnce, callKey, permissionKey, setupOutcome } from "./lib/gateway-view.mjs"
import { main } from "./lib/run.mjs"
import { integrationsFit, openIntegrations } from "./lib/settings.mjs"
import { css, names } from "./lib/selectors.mjs"
import { freePort, startDevServer } from "./lib/server.mjs"
import { settled } from "./lib/workspace.mjs"

/** The tool the Done-when step asks for, whose app is the chart (`server.mjs`). */
const CHART_TOOL = "show_chart"
const DESTRUCTIVE = "app_delete_row"
const RENAMED = `${SERVER}-renamed`
const VARIABLE = "MCP_TEST_SECRET"
/**
 * How long the walk's server holds its answer to `initialize`
 * (`--initialize-delay-ms`): an inspection runs at least this long, so its
 * running state is there to be read after the click, not raced.
 */
const START_DELAY_MS = 2000
const SLOW_ARGS = [serverScript, "--initialize-delay-ms", String(START_DELAY_MS)]

const steps = [
  "empty",
  "add",
  "inspect",
  "focus",
  "toggle",
  "rename",
  "relaunch",
  "narrow",
  "conflict",
  "remove",
  "reconnect",
  "too-large",
  "save-too-large",
  "duplicate-names",
  "non-admin",
  "done-when",
]

/**
 * The hand-edited list of the too-large steps: this many servers, each with
 * one long argument, written compact to just under config.json's 64 KiB.
 * The list answer carries more per server than the file does (`kind`,
 * `envNames`, `managed`), so the file fits and its list does not. One under
 * the SDK's 16 (`MAX_MCP_SERVERS`), so save-too-large's add is refused for
 * its size and not its count.
 */
const BIG_SERVERS = 15
/** What the file is left short of its 65 536-byte bound. */
const FILE_SLACK = 64
const FILE_LIMIT = 65_536
const bigName = (index) => `big-${String(index).padStart(2, "0")}`
/** The one removed by name, after which the list fits again. */
const BIG_REMOVED = bigName(7)
/** The server the refused save adds. */
const BIG_ADDED = "big-extra"

const meta = {
  name: "mcp-servers-gateway",
  summary:
    "Settings › Integrations over a real gateway: add, inspect, focus, toggle, rename, relaunch, conflict, remove, reconnect, a list too large to show, a save refused for size, two servers under one name, non-admin, and an app drawn from a server added there",
  defaults: { engine: "chromium,webkit", layout: "columns" },
  options: { only: { type: "string" }, agent: { type: "string", default: "claude" } },
  help: `
Usage: node verification/desktop/scripts/mcp-servers-gateway.mjs [options]

Needs: the gateway built (cargo build -p nessa-server; or MCP_LIVE_NESSA) and
the agent's harness installed (crates/nessa-sdk/harnesses/<agent>-acp, or
MCP_LIVE_HARNESSES); done-when needs the agent signed in on this machine. It
starts its own gateway and dev server; --url and --mode are not used.

Options:
  --agent claude|codex  the agent the gateway runs (default: claude)

Steps, per engine, in order on one page (--only <names> to pick):
  empty      no server stored: "No servers yet", Add offered
  add        ${SERVER} added with one variable, slow to start (${START_DELAY_MS} ms): one
             row, its switch on, "1 variable", the variable's value nowhere
             in the page
  inspect    seen running, Inspect and Close resting while it runs; then
             show_chart has a UI badge, ${DESTRUCTIVE} is destructive, the
             answer is complete
  focus      document.activeElement after each part opens and closes: Add and
             Edit on the form's first field, Remove on the confirm's Cancel
             (described by its sentence), Inspect on the panel's heading;
             Cancel, Escape, Save and Close back on the row's button or Add
  toggle     the switch turns it off: one save, the switch resting in flight,
             and off as the new list says
  rename     renamed to ${RENAMED}: one row, its variable kept
  relaunch   the gateway refuses another command with a value kept
             (environment_value_missing, from a Node client); in the window,
             another command asks for the value again, says why, and holds
             Save until it is typed; saved, the row shows the new command and
             still one variable, the value nowhere in the page
  narrow     with the row, the form and the inspection open, at 800 and 390px:
             nothing outside its card, no sideways scroll, the fold held, the
             row's actions under its text under a 420px page
  conflict   a Node client turns it back on first; the window's save is
             refused, says so, reloads, keeps what was typed, and refills
             the switch it left untouched from the reload
  remove     asked first while an inspection runs, then removed: the row
             gone, "No servers yet", the inspection saying the server is gone
  too-large  config.json hand-edited to ${BIG_SERVERS} servers with long arguments,
             under its 64 KiB, whose list will not fit one frame (a Node
             client's list refused mcp_servers_config_too_large): the panel
             says the list is too large to show, no row and no Add; ${BIG_REMOVED}
             removed by name, asked first: one remove, one list, and the list
             shown again without it (U44, U45)
  save-too-large
             a server added with an argument that would push the list past
             the bound, the file still under its own: refused, the form kept
             open with the sentence why, nothing listed again and config.json
             unchanged (U48); then config.json restored
  reconnect  config.json made unreadable, the socket dropped: the list's
             failure is said; config.json restored, the socket dropped again:
             the list shown and the failure's notice gone
  duplicate-names
             config.json edited to store two servers under one name: each row
             says so, Edit and the switch disabled, Inspect and Remove kept;
             Remove asked from the second row asks there only, saying the
             first stored goes, Cancel puts focus back on that row's Remove;
             confirmed: one remove, one list, the gateway keeps the second,
             focus on the row left (D2–D7); then config.json restored
  non-admin  a credential that may only converse (server.read and
             conversation.*) sends one list, is refused it, and sees the
             administrator notice and no control (U2)
  done-when  (once, after both engines) ${SERVER} added again from the window;
             the agent calls show_chart in a new conversation; in each engine
             its app frame renders the chart, once`,
}

/** The gateway, the dev server in front of it, and an owner client in Node. */
async function startStack(options) {
  const agent = options.agent
  if (!["claude", "codex"].includes(agent))
    throw new CannotRun(`--agent ${agent}: claude or codex`)
  let command
  try {
    command = agentCommand(agent)
  } catch (error) {
    throw new CannotRun(error.message)
  }
  const timings = {}
  let started = Date.now()
  const gateway = await startLocalGateway({
    agent,
    port: await freePort(),
    instance: "mcp-servers-gateway",
    agentArgv: command.argv,
    model: command.model,
    path: command.path,
    mcpServers: [],
  }).catch((error) => {
    if (error.gatewayLog) log(error.gatewayLog.slice(-4000))
    throw new CannotRun(`the gateway did not start: ${error.message.split("\n")[0]}`)
  })
  timings.gatewayMs = Date.now() - started
  let dev = null
  let client = null
  const close = async () => {
    client?.close()
    await dev?.close()
    if (!(await gateway.stop()))
      log(`the gateway (pid ${gateway.server.pid}) did not exit`)
  }
  try {
    started = Date.now()
    dev = await startDevServer(options, {
      NESSA_STAGE: "ci",
      VITE_NESSA_STAGE: "ci",
      NESSA_BROWSER_GATEWAY_URL: gateway.url,
    })
    timings.devServerMs = Date.now() - started
    const { register } = await import("tsx/esm/api")
    register()
    const { NessaClient } = await import("@nessa/client")
    const token = () => readFileSync(gateway.token, "utf8").trim()
    client = await NessaClient.connect({
      stage: "ci",
      url: gateway.url.replace(/^http/, "ws"),
      role: "surface",
      surface: { kind: "panel", instance: "mcp-servers-gateway" },
      client: { id: "mcp-servers-gateway", version: "0.1.0", platform: "node" },
      profile: "product",
      auth: { credential: token() },
    })
    const config = join(
      gateway.directory,
      "ci",
      "instances",
      "mcp-servers-gateway",
      "config.json",
    )
    // Another path to the same node, for the relaunch step's new command.
    const relaunch = join(gateway.directory, "node-again")
    if (!existsSync(relaunch)) symlinkSync(process.execPath, relaunch)
    return {
      url: dev.url,
      mode: "dev",
      close,
      client,
      agent,
      timings,
      token,
      config,
      relaunch,
    }
  } catch (error) {
    await close()
    throw error
  }
}

/**
 * A credential for this gateway's organization that may converse and nothing
 * more: `server.read`, which a browser session asks before it opens, and the
 * conversation grants. No `credential.manage`.
 */
async function conversingCredential(client) {
  const { organizationId, gatewayId } = client.productSession
  const id = `settings-reader-${randomUUID()}`
  const resource = { organizationId, id: gatewayId }
  const issued = await client.credentials.issue({
    principal: { id, kind: "integration" },
    membership: {
      id: `${id}-membership`,
      principalId: id,
      organizationId,
      role: "member",
      state: "active",
    },
    expiresAt: Math.floor(Date.now() / 1000) + 3600,
    grants: [
      { action: "server.read", resource },
      { action: "conversation.read", resource },
      { action: "conversation.write", resource },
    ],
  })
  if (!("secret" in issued))
    throw new CannotRun("the conversing credential has no secret")
  return issued.secret
}

/**
 * What a page's load reports that is not the page's fault: Chromium reports
 * each `/browser/check` the load sends, and the navigation abandons, as
 * aborted. That one line is set aside; every other error stays, and fails
 * the step it lands in.
 */
const loadAbort = /^requestfailed: \S+\/browser\/check net::ERR_ABORTED\s*$/

function setAsideLoadAbort(errors) {
  errors.splice(0, Infinity, ...errors.filter((each) => !loadAbort.test(each)))
}

/**
 * The page's socket to the gateway (`/browser/session`), routed through the
 * script: each mcpServers request sent on it counted (`sent`), and `drop()`
 * closing every one open at both ends, as a lost connection would.
 */
function routedSockets() {
  const sent = []
  const open = new Set()
  const route = (ws) => {
    const server = ws.connectToServer()
    const pair = { ws, server }
    open.add(pair)
    ws.onMessage((message) => {
      try {
        const frame = JSON.parse(
          typeof message === "string" ? message : message.toString(),
        )
        if (typeof frame.method === "string" && frame.method.startsWith("mcpServers."))
          sent.push(frame.method)
      } catch {
        // Not a JSON frame: not a request, so not one counted.
      }
      server.send(message)
    })
    server.onMessage((message) => ws.send(message))
    ws.onClose((code, reason) => {
      open.delete(pair)
      server.close({ code, reason })
    })
    server.onClose((code, reason) => {
      open.delete(pair)
      ws.close({ code, reason })
    })
  }
  // The page is told the gateway is restarting (1012): a close it retries,
  // as it would a gateway that went away and came back.
  const drop = async () => {
    const pairs = [...open]
    open.clear()
    for (const { ws, server } of pairs) {
      await server.close({ code: 1000 })
      await ws.close({ code: 1012 })
    }
    return pairs.length
  }
  return { sent, route, drop }
}

/**
 * A page signed in with `token` through `/browser/login` from the page's own
 * origin, on the gateway's window, its socket routed (`routedSockets`).
 */
async function signedIn(browser, stack, token, layout) {
  const sockets = routedSockets()
  const opened = await openPage(browser, {
    url: stack.url,
    layout,
    beforeLoad: (context) => context.routeWebSocket(/\/browser\/session/, sockets.route),
  })
  const { page } = opened
  // Its body read to the end, so the navigation after it abandons nothing.
  const status = await page.evaluate(async (token) => {
    const answer = await fetch("/browser/login", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", "X-Nessa-Browser": "1" },
      body: JSON.stringify({ token }),
    })
    await answer.arrayBuffer()
    return answer.status
  }, token)
  if (status < 200 || status > 299) {
    await opened.close()
    throw new CannotRun(`the gateway refused the page's sign-in (${status})`)
  }
  await page.goto(`${stack.url}?gateway`, { waitUntil: "domcontentloaded" })
  await need(page, css.anyReady, "the desktop window", 30_000)
  setAsideLoadAbort(opened.errors)
  return { ...opened, sent: sockets.sent, drop: sockets.drop }
}

/** Waits up to `ms` for `check()` to be truthy, and says what it last was. */
async function waitFor(check, ms) {
  const end = Date.now() + ms
  let value
  do {
    value = await check()
    if (value) return value
    await sleep(100)
  } while (Date.now() < end)
  return value
}

const visible = (locator, ms = 10_000) =>
  locator
    .first()
    .waitFor({ state: "visible", timeout: ms })
    .then(() => true)
    .catch(() => false)
const gone = (locator, ms = 10_000) =>
  locator
    .first()
    .waitFor({ state: "detached", timeout: ms })
    .then(() => true)
    .catch(() => false)

/** What the page holds anywhere: its markup, and every field's value. */
const pageHolds = (page, text) =>
  page.evaluate(
    (text) =>
      document.documentElement.outerHTML.includes(text) ||
      [...document.querySelectorAll("input, textarea")].some((field) =>
        field.value.includes(text),
      ),
    text,
  )

const stored = (page) => page.locator(css.mcpStoredRow)
const row = (page, name) => page.locator(css.mcpRowNamed(name))
const button = (scope, name) => scope.getByRole("button", { name, exact: true })
const form = (page) => page.locator(css.mcpForm)

/** Types `value` as a new argument, after those the form holds. */
async function addArgument(scope, value) {
  await button(scope, names.mcp.addArgument).click()
  await scope.locator(css.mcpArgument).last().locator("textarea").fill(value)
}

/** The form's arguments, as its fields hold them. */
const argumentsOf = (scope) =>
  scope
    .locator(`${css.mcpArgument} textarea`)
    .evaluateAll((fields) => fields.map((field) => field.value))

/**
 * Fills the add form and saves it: `args`, one field each, and `secret` as
 * one variable's value, when given.
 */
async function addServer(page, { secret, args = [serverScript] } = {}) {
  await button(page.locator(css.mcpGroup), names.mcp.add).click()
  const add = form(page)
  await add.getByLabel(names.mcp.name, { exact: true }).fill(SERVER)
  await add.getByLabel(names.mcp.command, { exact: true }).fill(process.execPath)
  for (const value of args) await addArgument(add, value)
  if (secret !== undefined) {
    await button(add, names.mcp.addVariable).click()
    const variable = add.locator(css.mcpVariable).last()
    await variable.getByLabel(names.mcp.variableName, { exact: true }).fill(VARIABLE)
    await variable.locator(css.mcpSecret).fill(secret)
  }
  await button(add, names.mcp.save).click()
}

/** The switch's state in a row, and whether it rests. */
const switchOf = (page, name) =>
  row(page, name)
    .locator(css.mcpSwitch)
    .evaluate((element) => ({
      checked: element.getAttribute("aria-checked"),
      disabled: element.disabled,
    }))

/**
 * What holds focus, in the tab's words: the button by what it does and the
 * row it is in, the form's first field, or the inspection's heading.
 */
const focused = (page) =>
  page.evaluate(() => {
    const active = document.activeElement
    if (!active || active === document.body) return { on: "body" }
    const form = active.closest("[data-mcp-form]")
    const firstField = form?.querySelector("input")
    return {
      on: active.getAttribute("data-mcp-action")
        ? `${active.getAttribute("data-mcp-action")}`
        : active === firstField
          ? "first-field"
          : active.matches("[data-mcp-inspection] h2")
            ? "inspection-heading"
            : `${active.tagName.toLowerCase()} ${active.textContent?.trim().slice(0, 40) ?? ""}`,
      row: active.closest("[data-mcp-server]")?.getAttribute("data-mcp-server") ?? null,
      // The stored row's place: two rows may share a name.
      at: active.closest("[data-mcp-row]")?.getAttribute("data-mcp-row") ?? null,
      describedBy: active.getAttribute("aria-describedby")
        ? (document.getElementById(active.getAttribute("aria-describedby"))
            ?.textContent ?? null)
        : null,
    }
  })

/**
 * `original` (config.json's bytes) with its stored servers replaced by
 * BIG_SERVERS servers, each one long argument, padded so the compact file is
 * FILE_SLACK bytes short of its bound. Each server is off, so none is started.
 */
function bigConfig(original) {
  const document = JSON.parse(original.toString("utf8"))
  const write = (padding) => {
    document.agents.mcpServers = Array.from({ length: BIG_SERVERS }, (_, index) => ({
      name: bigName(index),
      command: process.execPath,
      args: [serverScript, "p".repeat(padding)],
      enabled: false,
    }))
    return Buffer.from(`${JSON.stringify(document)}\n`)
  }
  const bare = write(0).length
  const padding = Math.floor((FILE_LIMIT - FILE_SLACK - bare) / BIG_SERVERS)
  return { bytes: write(padding), padding, document }
}

/** The name the duplicate-names step stores twice, by hand. */
const DUPLICATE = "twice"

/** config.json with `DUPLICATE` stored twice: the first on, the second off. */
function duplicateConfig(original) {
  const document = JSON.parse(original.toString("utf8"))
  document.agents.mcpServers = [
    { name: DUPLICATE, command: process.execPath, args: [serverScript, "first"] },
    {
      name: DUPLICATE,
      command: process.execPath,
      args: [serverScript, "second"],
      enabled: false,
    },
  ]
  return Buffer.from(`${JSON.stringify(document, null, 2)}\n`)
}

/** The refusal a Node client's list gets, or `null` when it lists. */
const listRefusal = (client) =>
  client.mcpServers.list().then(
    () => null,
    (error) => error.refusal ?? { code: String(error) },
  )

const checks = {
  empty: async (page) => {
    const failures = []
    if (!(await visible(page.locator(css.mcpServersIn("listed")), 20_000)))
      failures.push(
        `the tab is ${await page.locator(css.mcpServers).getAttribute("data-mcp-servers")}, not listed`,
      )
    const seen = {
      stored: await stored(page).count(),
      managed: await page.locator(css.mcpManagedRow).count(),
      empty: (await page.locator(css.mcpEmpty).textContent())?.includes(names.mcp.empty),
      addEnabled: await button(page.locator(css.mcpGroup), names.mcp.add).isEnabled(),
    }
    if (seen.stored !== 0) failures.push(`${seen.stored} stored rows, expected none`)
    if (!seen.empty) failures.push(`no "${names.mcp.empty}"`)
    if (!seen.addEnabled) failures.push("Add server… is disabled")
    return { seen, failures }
  },

  add: async (page, stack, context) => {
    const failures = []
    context.secret = `s3cret-${randomUUID()}`
    const before = context.opened.sent.length
    const at = Date.now()
    await addServer(page, { secret: context.secret, args: SLOW_ARGS })
    const closed = await gone(form(page))
    const shown = await visible(row(page, SERVER))
    await settled(page)
    const seen = {
      ms: Date.now() - at,
      formClosed: closed,
      rows: await stored(page).count(),
      switch: shown ? await switchOf(page, SERVER) : null,
      variables: shown ? await row(page, SERVER).locator("small").textContent() : null,
      requests: context.opened.sent.slice(before),
      secretInPage: await pageHolds(page, context.secret),
    }
    if (!closed) failures.push("the form is still open after the save")
    if (seen.rows !== 1) failures.push(`${seen.rows} stored rows, expected 1`)
    if (seen.switch?.checked !== "true")
      failures.push(`the switch is ${JSON.stringify(seen.switch)}, not on`)
    if (seen.variables !== "1 variable") failures.push(`the row says "${seen.variables}"`)
    if (seen.secretInPage)
      failures.push("the variable's value is in the page after the save")
    if (
      JSON.stringify(seen.requests) !==
      JSON.stringify(["mcpServers.save", "mcpServers.list"])
    )
      failures.push(
        `requests ${JSON.stringify(seen.requests)}, expected one save then one list`,
      )
    return { seen, failures }
  },

  inspect: async (page) => {
    const failures = []
    const at = Date.now()
    await button(row(page, SERVER), names.mcp.inspect).click()
    // The server holds its start for START_DELAY_MS: the panel is running
    // well inside that, and what rests while it runs is read then.
    const running = await visible(page.locator(css.mcpInspectionIn("running")), 1000)
    const whileRunning = running
      ? {
          inspectRests: await button(row(page, SERVER), names.mcp.inspect).isDisabled(),
          closeRests: await button(
            page.locator(css.mcpInspection),
            names.mcp.close,
          ).isDisabled(),
          stillRunning:
            (await page.locator(css.mcpInspectionIn("running")).count()) === 1,
          ms: Date.now() - at,
        }
      : null
    const done = await visible(page.locator(css.mcpInspectionIn("done")), 45_000)
    const panel = page.locator(css.mcpInspection)
    const seen = {
      ms: Date.now() - at,
      phase: await panel.getAttribute("data-mcp-inspection"),
      whileRunning,
      chartUi: await panel
        .locator(`${css.mcpTool(CHART_TOOL)} ${css.mcpBadge("ui")}`)
        .count(),
      destructive: await panel
        .locator(`${css.mcpTool(DESTRUCTIVE)} ${css.mcpBadge("destructive")}`)
        .count(),
      cut: await panel.locator(css.mcpCut).count(),
      tools: await panel.locator("[data-mcp-tool]").count(),
    }
    if (!running)
      failures.push("the inspection was not seen running within 1 s of the click")
    else if (!whileRunning.stillRunning)
      failures.push(`the inspection finished before it was read (${whileRunning.ms} ms)`)
    else {
      if (!whileRunning.inspectRests) failures.push("Inspect is enabled while running")
      if (!whileRunning.closeRests) failures.push("Close is enabled while running")
    }
    if (!done) failures.push(`the inspection is ${seen.phase}, not done within 45 s`)
    if (seen.ms < START_DELAY_MS)
      failures.push(
        `the inspection took ${seen.ms} ms, under the server's ${START_DELAY_MS} ms start`,
      )
    if (seen.chartUi !== 1) failures.push(`${CHART_TOOL} has ${seen.chartUi} UI badges`)
    if (seen.destructive !== 1) failures.push(`${DESTRUCTIVE} is not marked destructive`)
    if (seen.cut !== 0) failures.push("the inspection says it is incomplete")
    await button(panel, names.mcp.close).click()
    if (!(await gone(panel))) failures.push("the inspection did not close")
    return { seen, failures }
  },

  focus: async (page) => {
    const failures = []
    const trail = []
    const group = page.locator(css.mcpGroup)
    const serverRow = row(page, SERVER)
    /** Where focus is after `what`, held to `on` (and its row, when given). */
    const expect = async (what, on, inRow = null) => {
      const at = await waitFor(async () => {
        const now = await focused(page)
        return now.on === on && now.row === inRow ? now : null
      }, 5000)
      const now = at ?? (await focused(page))
      trail.push({ after: what, ...now })
      if (!at)
        failures.push(
          `after ${what}, focus is on ${now.on}${now.row ? ` in ${now.row}` : ""}, not ${on}${inRow ? ` in ${inRow}` : ""}`,
        )
      return now
    }
    await button(group, names.mcp.add).click()
    await expect("Add", "first-field")
    // A part Escape leaves open holds the rest of the walk: closed by its
    // Cancel, and the step ends there.
    await page.keyboard.press("Escape")
    if (!(await gone(form(page), 2000))) {
      failures.push("Escape did not close the add form")
      await button(form(page), names.mcp.cancel).click()
      return { seen: { trail }, failures }
    }
    await expect("Escape in the add form", "add")
    await button(serverRow, names.mcp.edit).click()
    await expect("Edit", "first-field")
    await button(form(page), names.mcp.cancel).click()
    await expect("Cancel in the form", "edit", SERVER)
    await button(serverRow, names.mcp.remove).click()
    const asked = await expect("Remove", "cancel", SERVER)
    if (asked.describedBy !== names.mcp.removeAsk(SERVER))
      failures.push(`the confirm's Cancel is described by "${asked.describedBy}"`)
    await page.keyboard.press("Escape")
    if (!(await gone(serverRow.locator(css.mcpConfirm), 2000))) {
      failures.push("Escape did not close the confirm")
      await serverRow.locator(css.mcpAction("cancel")).click()
      return { seen: { trail }, failures }
    }
    await expect("Escape in the confirm", "remove", SERVER)
    await button(serverRow, names.mcp.remove).click()
    await expect("Remove again", "cancel", SERVER)
    await serverRow.locator(css.mcpAction("cancel")).click()
    await expect("Cancel in the confirm", "remove", SERVER)
    await button(serverRow, names.mcp.inspect).click()
    await expect("Inspect", "inspection-heading")
    if (!(await visible(page.locator(css.mcpInspectionIn("done")), 45_000)))
      failures.push("the inspection did not finish")
    await button(page.locator(css.mcpInspection), names.mcp.close).click()
    await expect("Close", "inspect", SERVER)
    // A save goes back to the row's Edit once the list after it is read.
    await button(serverRow, names.mcp.edit).click()
    await expect("Edit again", "first-field")
    await button(form(page), names.mcp.save).click()
    if (!(await gone(form(page)))) failures.push("the form did not close on its save")
    await expect("Save", "edit", SERVER)
    await settled(page)
    return { seen: { trail }, failures }
  },

  toggle: async (page, stack, context) => {
    const failures = []
    const before = context.opened.sent.length
    const toggle = row(page, SERVER).locator(css.mcpSwitch)
    // Whether the switch rested at any moment while its save was in flight,
    // and Inspect with it (U21: every control, while one request runs).
    await toggle.evaluate((element) => {
      const inspect = element
        .closest("[data-mcp-server]")
        .querySelector('[data-mcp-action="inspect"]')
      window.__mcpSwitchRested = false
      window.__mcpInspectRested = false
      new MutationObserver(() => {
        if (!element.disabled) return
        window.__mcpSwitchRested = true
        if (inspect.disabled) window.__mcpInspectRested = true
      }).observe(element, { attributes: true })
    })
    await toggle.click()
    const off = await waitFor(
      async () => (await switchOf(page, SERVER)).checked === "false",
      10_000,
    )
    await settled(page)
    const seen = {
      switch: await switchOf(page, SERVER),
      restedInFlight: await page.evaluate(() => window.__mcpSwitchRested),
      inspectRestedInFlight: await page.evaluate(() => window.__mcpInspectRested),
      requests: context.opened.sent.slice(before),
    }
    if (!off) failures.push(`the switch is ${JSON.stringify(seen.switch)}, not off`)
    if (!seen.restedInFlight)
      failures.push("the switch did not rest while its save was in flight")
    else if (!seen.inspectRestedInFlight)
      failures.push("Inspect did not rest while the switch's save was in flight")
    if (
      JSON.stringify(seen.requests) !==
      JSON.stringify(["mcpServers.save", "mcpServers.list"])
    )
      failures.push(
        `requests ${JSON.stringify(seen.requests)}, expected one save then one list`,
      )
    return { seen, failures }
  },

  rename: async (page, stack, context) => {
    const failures = []
    await button(row(page, SERVER), names.mcp.edit).click()
    const edit = form(page)
    const placeholder = await edit.locator(css.mcpSecret).getAttribute("placeholder")
    await edit.getByLabel(names.mcp.name, { exact: true }).fill(RENAMED)
    await button(edit, names.mcp.save).click()
    const closed = await gone(edit)
    const shown = await visible(row(page, RENAMED))
    await settled(page)
    const seen = {
      placeholder,
      rows: await stored(page).evaluateAll((rows) =>
        rows.map((each) => each.getAttribute("data-mcp-server")),
      ),
      variables: shown ? await row(page, RENAMED).locator("small").textContent() : null,
      secretInPage: await pageHolds(page, context.secret),
    }
    if (placeholder !== names.mcp.storedValue)
      failures.push(`the stored value's placeholder is "${placeholder}"`)
    if (!closed) failures.push("the form is still open after the save")
    if (JSON.stringify(seen.rows) !== JSON.stringify([RENAMED]))
      failures.push(`rows ${JSON.stringify(seen.rows)}, expected [${RENAMED}]`)
    if (seen.variables !== "1 variable") failures.push(`the row says "${seen.variables}"`)
    if (seen.secretInPage) failures.push("the variable's value is in the page")
    return { seen, failures }
  },

  relaunch: async (page, stack, context) => {
    const failures = []
    // The gateway's rule first: another command with the value kept is refused.
    const listed = await stack.client.mcpServers.list()
    const server = listed.servers.find((each) => each.name === RENAMED)
    if (!server) throw new CannotRun(`the gateway lists no ${RENAMED}`)
    const refused = await stack.client.mcpServers
      .save({
        revision: listed.revision,
        server: {
          kind: server.kind,
          name: server.name,
          command: stack.relaunch,
          args: server.args,
          env: server.envNames.map((name) => ({ name, value: null })),
          enabled: server.enabled,
        },
      })
      .then(
        () => null,
        (error) => error.refusal ?? { code: String(error) },
      )
    // In the window: the value asked for again, and Save held until typed.
    await button(row(page, RENAMED), names.mcp.edit).click()
    const edit = form(page)
    const secret = edit.locator(css.mcpSecret)
    const note = edit.locator(css.mcpValuesNeeded)
    const save = button(edit, names.mcp.save)
    const before = {
      placeholder: await secret.getAttribute("placeholder"),
      note: await note.textContent(),
      saveEnabled: await save.isEnabled(),
    }
    await edit.locator(css.mcpField("command")).fill(stack.relaunch)
    const changed = {
      placeholder: await secret.getAttribute("placeholder"),
      note: await note.textContent(),
      saveEnabled: await save.isEnabled(),
    }
    await secret.fill(context.secret)
    const typed = { note: await note.textContent(), saveEnabled: await save.isEnabled() }
    const sentFrom = context.opened.sent.length
    await save.click()
    const closed = await gone(edit)
    await settled(page)
    const after = await stack.client.mcpServers.list()
    const now = after.servers.find((each) => each.name === RENAMED)
    const seen = {
      refused,
      before,
      changed,
      typed,
      command: await row(page, RENAMED).locator("code").textContent(),
      variables: await row(page, RENAMED).locator("small").textContent(),
      listed: now && { command: now.command, envNames: now.envNames },
      requests: context.opened.sent.slice(sentFrom),
      secretInPage: await pageHolds(page, context.secret),
    }
    if (
      refused?.code !== "mcp_servers_invalid" ||
      refused.details?.problem !== "environment_value_missing"
    )
      failures.push(
        `the gateway answered a kept value under another command with ${JSON.stringify(refused)}`,
      )
    if (
      before.placeholder !== names.mcp.storedValue ||
      before.note !== "" ||
      !before.saveEnabled
    )
      failures.push(`before the change: ${JSON.stringify(before)}`)
    if (
      changed.placeholder !== names.mcp.storedValueAgain ||
      changed.note !== names.mcp.valuesAgain ||
      changed.saveEnabled
    )
      failures.push(`with another command: ${JSON.stringify(changed)}`)
    if (typed.note !== "" || !typed.saveEnabled)
      failures.push(`with the value typed again: ${JSON.stringify(typed)}`)
    if (!closed) failures.push("the form is still open after the save")
    if (!seen.command?.startsWith(stack.relaunch))
      failures.push(`the row shows "${seen.command}", not the new command`)
    if (seen.variables !== "1 variable") failures.push(`the row says "${seen.variables}"`)
    if (
      JSON.stringify(seen.listed) !==
      JSON.stringify({ command: stack.relaunch, envNames: [VARIABLE] })
    )
      failures.push(`the gateway lists ${JSON.stringify(seen.listed)}`)
    if (
      JSON.stringify(seen.requests) !==
      JSON.stringify(["mcpServers.save", "mcpServers.list"])
    )
      failures.push(
        `requests ${JSON.stringify(seen.requests)}, expected one save then one list`,
      )
    if (seen.secretInPage) failures.push("the variable's value is in the page")
    return { seen, failures }
  },

  narrow: async (page) => {
    // The row, the form and the finished inspection all open, then measured.
    await button(row(page, RENAMED), names.mcp.inspect).click()
    if (!(await visible(page.locator(css.mcpInspectionIn("done")), 45_000)))
      return {
        failures: [
          `the inspection is ${await page.locator(css.mcpInspection).getAttribute("data-mcp-inspection")}, not done within 45 s: nothing measured`,
        ],
      }
    await button(row(page, RENAMED), names.mcp.edit).click()
    await need(page, css.mcpForm, "the edit form")
    const { seen, failures } = await integrationsFit(page, [800, 390])
    await page.setViewportSize({ width: 1440, height: 900 })
    await settled(page)
    await button(form(page), names.mcp.cancel).click()
    await button(page.locator(css.mcpInspection), names.mcp.close).click()
    await gone(page.locator(css.mcpInspection))
    return { seen: { widths: seen }, failures }
  },

  conflict: async (page, stack, context) => {
    const failures = []
    // Another writer first: the window still holds the revision before it.
    const listed = await stack.client.mcpServers.list()
    const server = listed.servers.find((each) => each.name === RENAMED)
    if (!server) throw new CannotRun(`the gateway lists no ${RENAMED}`)
    await stack.client.mcpServers.save({
      revision: listed.revision,
      server: {
        kind: server.kind,
        name: server.name,
        command: server.command,
        args: server.args,
        env: server.envNames.map((name) => ({ name, value: null })),
        enabled: !server.enabled,
      },
    })
    const before = context.opened.sent.length
    await button(row(page, RENAMED), names.mcp.edit).click()
    const edit = form(page)
    const formSwitch = () =>
      edit
        .locator(css.mcpSwitch)
        .evaluate((element) => element.getAttribute("aria-checked"))
    const switchBefore = await formSwitch()
    // Another argument is another launch: the value is typed again (U33).
    await addArgument(edit, "--typed")
    await edit.locator(css.mcpSecret).fill(context.secret)
    await button(edit, names.mcp.save).click()
    const said = await waitFor(
      async () => (await page.locator(css.mcpNotice).allTextContents()).join(" "),
      10_000,
    )
    await waitFor(async () => (await switchOf(page, RENAMED)).checked === "true", 5000)
    const seen = {
      notice: said,
      formOpen: (await form(page).count()) === 1,
      kept: await argumentsOf(edit).catch(() => null),
      formSwitch: { before: switchBefore, after: await formSwitch().catch(() => null) },
      switch: await switchOf(page, RENAMED),
      requests: context.opened.sent.slice(before),
    }
    if (!seen.notice.includes(names.mcp.conflict))
      failures.push(`the window says "${seen.notice}", not the conflict`)
    if (!seen.formOpen) failures.push("the form closed on the conflict")
    if (JSON.stringify(seen.kept) !== JSON.stringify([...server.args, "--typed"]))
      failures.push(`the arguments are ${JSON.stringify(seen.kept)}, not as typed`)
    // Untouched in the form, the switch follows the other writer (U42).
    if (seen.formSwitch.before !== "false" || seen.formSwitch.after !== "true")
      failures.push(
        `the form's switch went ${JSON.stringify(seen.formSwitch)}, not off to the other writer's on`,
      )
    // The other writer turned it back on: the reload shows it.
    if (seen.switch.checked !== "true")
      failures.push(
        `the reloaded switch is ${seen.switch.checked}, not the other writer's on`,
      )
    if (
      JSON.stringify(seen.requests) !==
      JSON.stringify(["mcpServers.save", "mcpServers.list"])
    )
      failures.push(
        `requests ${JSON.stringify(seen.requests)}, expected one save then one list`,
      )
    await button(form(page), names.mcp.cancel).click()
    return { seen, failures }
  },

  remove: async (page, stack, context) => {
    const failures = []
    // Inspected first, and removed while the inspection still runs (the
    // server is slow to start): the panel then says the server is gone.
    await button(row(page, RENAMED), names.mcp.inspect).click()
    const inspecting = await visible(page.locator(css.mcpInspectionIn("running")), 1000)
    const before = context.opened.sent.length
    await button(row(page, RENAMED), names.mcp.remove).click()
    const asked = await row(page, RENAMED).locator(css.mcpConfirm).textContent()
    const sentOnAsk = context.opened.sent.length - before
    await row(page, RENAMED).locator(css.mcpAction("confirm")).click()
    const removed = await gone(row(page, RENAMED))
    const empty = await visible(page.locator(css.mcpEmpty))
    const stillRunning =
      (await page.locator(css.mcpInspectionIn("running")).count()) === 1
    const failed = await visible(page.locator(css.mcpInspectionIn("failed")), 45_000)
    const seen = {
      inspecting,
      stillRunningAfterRemoval: stillRunning,
      asked,
      sentOnAsk,
      removed,
      empty,
      inspection: failed
        ? await page.locator("[data-mcp-inspection-status]").textContent()
        : await page.locator(css.mcpInspection).getAttribute("data-mcp-inspection"),
      requests: context.opened.sent.slice(before),
    }
    if (!inspecting) failures.push("the inspection was not seen running")
    if (!stillRunning)
      failures.push("the inspection finished before the removal's list: nothing raced")
    if (seen.inspection !== names.mcp.gone(RENAMED))
      failures.push(`the inspection of the removed server says "${seen.inspection}"`)
    await button(page.locator(css.mcpInspection), names.mcp.close).click()
    if (!(await gone(page.locator(css.mcpInspection))))
      failures.push("the inspection did not close")
    if (asked !== names.mcp.removeAsk(RENAMED)) failures.push(`it asked "${asked}"`)
    if (sentOnAsk !== 0)
      failures.push(`${sentOnAsk} requests sent before the removal was confirmed`)
    if (!removed) failures.push("the row is still there")
    if (!empty) failures.push(`no "${names.mcp.empty}" after the removal`)
    if (
      JSON.stringify(seen.requests) !==
      JSON.stringify(["mcpServers.remove", "mcpServers.list"])
    )
      failures.push(
        `requests ${JSON.stringify(seen.requests)}, expected one remove then one list`,
      )
    return { seen, failures }
  },

  reconnect: async (page, stack, context) => {
    const failures = []
    const tab = page.locator(css.mcpServers)
    const notices = page.locator(css.mcpNotices)
    const original = readFileSync(stack.config)
    const seen = {}
    try {
      // A list that fails: config.json unreadable, and the window lists
      // again on its next connection.
      writeFileSync(stack.config, "{ not json")
      seen.droppedFirst = await context.opened.drop()
      seen.failed = await waitFor(
        async () =>
          (await tab.getAttribute("data-mcp-servers")) === "failed" &&
          (await notices.textContent())?.includes(names.mcp.configInvalid),
        20_000,
      )
      seen.failedNotice = await notices.textContent()
    } finally {
      writeFileSync(stack.config, original)
    }
    if (!seen.failed) {
      failures.push(
        `the failed list is not said: the tab is ${await tab.getAttribute("data-mcp-servers")}, its notices "${await notices.textContent()}"`,
      )
      return { seen, failures }
    }
    // Restored, and the connection lost and back: the list it reads answers
    // the failure, so the failure's notice goes.
    seen.droppedSecond = await context.opened.drop()
    seen.listed = await visible(page.locator(css.mcpServersIn("listed")), 20_000)
    await settled(page)
    seen.connection = await tab.getAttribute("data-connection")
    seen.notices = await notices.textContent()
    seen.empty = await visible(page.locator(css.mcpEmpty), 1000)
    if (seen.droppedFirst < 1 || seen.droppedSecond < 1)
      failures.push(`dropped ${seen.droppedFirst} then ${seen.droppedSecond} sockets`)
    if (!seen.listed) failures.push("the list is not shown after the reconnect")
    if (seen.connection !== "connected") failures.push(`the tab is ${seen.connection}`)
    if (seen.notices !== "")
      failures.push(`after the reconnect's list, the notices still say "${seen.notices}"`)
    if (!seen.empty) failures.push(`no "${names.mcp.empty}" after the reconnect`)
    return { seen, failures }
  },

  "too-large": async (page, stack, context) => {
    const failures = []
    const tab = page.locator(css.mcpServers)
    const tooLarge = page.locator(css.mcpTooLarge)
    context.original = readFileSync(stack.config)
    const big = bigConfig(context.original)
    writeFileSync(stack.config, big.bytes)
    const seen = { fileBytes: big.bytes.length, padding: big.padding }
    // The premise, from the gateway itself: the file fits, its list does not.
    seen.nodeList = await listRefusal(stack.client)
    if (seen.fileBytes > FILE_LIMIT)
      throw new CannotRun(`the hand-edited file is ${seen.fileBytes} bytes`)
    if (seen.nodeList?.code !== "mcp_servers_config_too_large")
      throw new CannotRun(
        `the gateway answers the hand-edited list with ${JSON.stringify(seen.nodeList)}`,
      )
    // The window lists again on its next connection.
    seen.dropped = await context.opened.drop()
    seen.shown = await visible(page.locator(css.mcpServersIn("too-large")), 20_000)
    if (!seen.shown) {
      failures.push(
        `the tab is ${await tab.getAttribute("data-mcp-servers")}, not too-large; its notices "${await page.locator(css.mcpNotices).textContent()}"`,
      )
      return { seen, failures }
    }
    await settled(page)
    const field = tooLarge.locator(css.mcpAction("removeByName"))
    seen.sentence = await tooLarge.locator("p").first().textContent()
    seen.fieldDescribedBy = await field.evaluate(
      (element) =>
        document.getElementById(element.getAttribute("aria-describedby"))?.textContent ??
        null,
    )
    seen.rows = await page.locator(css.mcpRow).count()
    seen.add = await page.locator(css.mcpAction("add")).count()
    seen.removeBeforeName = await button(tooLarge, names.mcp.remove).isEnabled()
    if (seen.sentence !== names.mcp.listTooLarge)
      failures.push(`the panel says "${seen.sentence}"`)
    if (seen.fieldDescribedBy !== names.mcp.listTooLarge)
      failures.push(`the name field is described by "${seen.fieldDescribedBy}"`)
    if (seen.rows !== 0) failures.push(`${seen.rows} rows drawn`)
    if (seen.add !== 0) failures.push("Add server… is offered")
    if (seen.removeBeforeName) failures.push("Remove is enabled with no name typed")
    // Removed by name, asked first: nothing sent until confirmed.
    await field.fill(BIG_REMOVED)
    const before = context.opened.sent.length
    await button(tooLarge, names.mcp.remove).click()
    seen.asked = await tooLarge.locator(css.mcpConfirm).textContent()
    seen.sentOnAsk = context.opened.sent.length - before
    await tooLarge.locator(css.mcpAction("confirm")).click()
    seen.listed = await visible(page.locator(css.mcpServersIn("listed")), 20_000)
    await settled(page)
    seen.requests = context.opened.sent.slice(before)
    seen.listedRows = await stored(page).evaluateAll((rows) =>
      rows.map((each) => each.getAttribute("data-mcp-server")),
    )
    seen.notices = await page.locator(css.mcpNotices).textContent()
    const expected = Array.from({ length: BIG_SERVERS }, (_, index) =>
      bigName(index),
    ).filter((name) => name !== BIG_REMOVED)
    if (seen.asked !== names.mcp.removeAsk(BIG_REMOVED))
      failures.push(`it asked "${seen.asked}"`)
    if (seen.sentOnAsk !== 0)
      failures.push(`${seen.sentOnAsk} requests sent before the removal was confirmed`)
    if (!seen.listed)
      failures.push(
        `after the remove the tab is ${await tab.getAttribute("data-mcp-servers")}, not listed`,
      )
    if (JSON.stringify([...seen.listedRows].sort()) !== JSON.stringify(expected))
      failures.push(
        `rows ${JSON.stringify(seen.listedRows)}, expected the ${expected.length} but ${BIG_REMOVED}`,
      )
    if (seen.notices !== "") failures.push(`the notices say "${seen.notices}"`)
    if (
      JSON.stringify(seen.requests) !==
      JSON.stringify(["mcpServers.remove", "mcpServers.list"])
    )
      failures.push(
        `requests ${JSON.stringify(seen.requests)}, expected one remove then one list`,
      )
    return { seen, failures }
  },

  "save-too-large": async (page, stack, context) => {
    const failures = []
    if (!context.original) throw new CannotRun("not run: too-large did not run first")
    const seen = {}
    try {
      const fileBefore = readFileSync(stack.config)
      const stored = JSON.parse(fileBefore.toString("utf8"))
      const longest = Math.max(
        ...stored.agents.mcpServers.map((each) => each.args.at(-1).length),
      )
      // Shorter than the one removed, so the file it would write fits its
      // bound; with the list's per-server fields, the answer would not.
      const argument = "q".repeat(longest - 600)
      stored.agents.mcpServers.push({
        name: BIG_ADDED,
        command: process.execPath,
        args: [argument],
        enabled: true,
      })
      seen.fileWouldBe = Buffer.byteLength(`${JSON.stringify(stored)}\n`)
      if (seen.fileWouldBe > FILE_LIMIT)
        throw new CannotRun(
          `the save's file would be ${seen.fileWouldBe} bytes, past its own bound`,
        )
      const before = context.opened.sent.length
      await button(page.locator(css.mcpGroup), names.mcp.add).click()
      const add = form(page)
      await add.getByLabel(names.mcp.name, { exact: true }).fill(BIG_ADDED)
      await add.getByLabel(names.mcp.command, { exact: true }).fill(process.execPath)
      await addArgument(add, argument)
      await button(add, names.mcp.save).click()
      const problem = add.locator(`${css.mcpProblem}[data-mcp-problem="form"]`)
      seen.problem = await waitFor(async () => await problem.textContent(), 10_000)
      // Long enough for a list the window should not send to have gone.
      await sleep(1500)
      seen.formOpen = (await form(page).count()) === 1
      seen.kept = await argumentsOf(add).then((args) => args.map((each) => each.length))
      seen.requests = context.opened.sent.slice(before)
      seen.notices = await page.locator(css.mcpNotices).textContent()
      seen.fileUnchanged = readFileSync(stack.config).equals(fileBefore)
      seen.saveEnabled = await button(add, names.mcp.save).isEnabled()
      if (seen.problem !== names.mcp.saveTooLarge)
        failures.push(`the form says "${seen.problem}"`)
      if (!seen.formOpen) failures.push("the form closed on the refusal")
      if (JSON.stringify(seen.kept) !== JSON.stringify([argument.length]))
        failures.push(`the arguments kept are ${JSON.stringify(seen.kept)} long`)
      if (!seen.saveEnabled) failures.push("Save rests after the refusal")
      if (seen.notices !== "") failures.push(`the notices say "${seen.notices}"`)
      if (!seen.fileUnchanged) failures.push("config.json changed")
      if (JSON.stringify(seen.requests) !== JSON.stringify(["mcpServers.save"]))
        failures.push(
          `requests ${JSON.stringify(seen.requests)}, expected one save alone`,
        )
      await button(add, names.mcp.cancel).click()
      if (!(await gone(add))) failures.push("the form did not close on Cancel")
    } finally {
      // Back to the file before the too-large steps, and listed again.
      writeFileSync(stack.config, context.original)
      context.original = null
    }
    seen.dropped = await context.opened.drop()
    seen.restored =
      (await visible(page.locator(css.mcpServersIn("listed")), 20_000)) &&
      (await visible(page.locator(css.mcpEmpty), 5000))
    await settled(page)
    if (!seen.restored) failures.push("the restored list is not shown empty")
    return { seen, failures }
  },

  "duplicate-names": async (page, stack, context) => {
    const failures = []
    const tab = page.locator(css.mcpServers)
    const original = readFileSync(stack.config)
    const seen = {}
    try {
      writeFileSync(stack.config, duplicateConfig(original))
      // The premise, from the gateway itself: it lists both under one name.
      const premise = await stack.client.mcpServers.list()
      seen.nodeList = premise.servers.map((each) => [each.name, each.args.at(-1)])
      if (premise.servers.filter((each) => each.name === DUPLICATE).length !== 2)
        throw new CannotRun(
          `the gateway lists the hand-edited file as ${JSON.stringify(seen.nodeList)}`,
        )
      seen.dropped = await context.opened.drop()
      seen.listed = await waitFor(
        async () =>
          (await page.locator(css.mcpRowNamed(DUPLICATE)).count()) === 2 &&
          (await tab.getAttribute("data-mcp-servers")) === "listed",
        20_000,
      )
      if (!seen.listed) {
        failures.push(
          `the tab is ${await tab.getAttribute("data-mcp-servers")} with ${await stored(page).count()} rows, not two named ${DUPLICATE}`,
        )
        return { seen, failures }
      }
      await settled(page)
      // D2: each row says why; neither edits nor switches; Inspect and Remove stay.
      seen.rows = []
      for (const at of [0, 1]) {
        const each = page.locator(css.mcpRowAt(at))
        const shown = {
          name: await each.getAttribute("data-mcp-server"),
          shared: await each
            .locator(css.mcpShared)
            .textContent()
            .catch(() => null),
          edit: await button(each, names.mcp.edit).isEnabled(),
          toggle: await each.locator(css.mcpSwitch).isEnabled(),
          inspect: await button(each, names.mcp.inspect).isEnabled(),
          remove: await button(each, names.mcp.remove).isEnabled(),
        }
        seen.rows.push(shown)
        if (shown.name !== DUPLICATE) failures.push(`row ${at} is ${shown.name}`)
        if (shown.shared !== names.mcp.nameShared)
          failures.push(`row ${at} says "${shown.shared}"`)
        if (shown.edit) failures.push(`row ${at}'s Edit is enabled`)
        if (shown.toggle) failures.push(`row ${at}'s switch is enabled`)
        if (!shown.inspect) failures.push(`row ${at}'s Inspect is disabled`)
        if (!shown.remove) failures.push(`row ${at}'s Remove is disabled`)
      }
      // D4, D5: asked from the second row only; Cancel puts focus back there.
      const second = page.locator(css.mcpRowAt(1))
      const before = context.opened.sent.length
      await button(second, names.mcp.remove).click()
      seen.asked = await second.locator(css.mcpConfirm).textContent()
      seen.firstAsked = await page
        .locator(css.mcpRowAt(0))
        .locator(css.mcpConfirm)
        .count()
      seen.onAsk = await focused(page)
      await button(second, names.mcp.cancel).click()
      seen.onCancel = await focused(page)
      if (seen.asked !== names.mcp.removeFirstAsk(DUPLICATE))
        failures.push(`row 1 asked "${seen.asked}"`)
      if (seen.firstAsked !== 0) failures.push("row 0 shows the confirm too")
      if (seen.onAsk.on !== "cancel" || seen.onAsk.at !== "1")
        failures.push(`asked, focus is on ${JSON.stringify(seen.onAsk)}`)
      if (seen.onCancel.on !== "remove" || seen.onCancel.at !== "1")
        failures.push(`cancelled, focus is on ${JSON.stringify(seen.onCancel)}`)
      // D6, D7: confirmed from the second row; the gateway removes the first stored.
      await button(second, names.mcp.remove).click()
      await second.locator(css.mcpAction("confirm")).click()
      seen.relisted = await waitFor(
        async () =>
          (await stored(page).count()) === 1 &&
          (await page.locator(css.mcpShared).count()) === 0 &&
          (await focused(page)).on === "remove",
        20_000,
      )
      await settled(page)
      seen.requests = context.opened.sent.slice(before)
      seen.onRemoved = await focused(page)
      const after = await stack.client.mcpServers.list()
      seen.left = after.servers
        .filter((each) => !each.managed)
        .map((each) => [each.name, each.args.at(-1), each.enabled])
      const only = page.locator(css.mcpRowAt(0))
      seen.editAfter = await button(only, names.mcp.edit).isEnabled()
      seen.toggleAfter = await only.locator(css.mcpSwitch).isEnabled()
      if (!seen.relisted) failures.push("the list did not come back with one row")
      if (
        JSON.stringify(seen.requests) !==
        JSON.stringify(["mcpServers.remove", "mcpServers.list"])
      )
        failures.push(
          `requests ${JSON.stringify(seen.requests)}, expected one remove then one list`,
        )
      if (JSON.stringify(seen.left) !== JSON.stringify([[DUPLICATE, "second", false]]))
        failures.push(`the gateway kept ${JSON.stringify(seen.left)}`)
      if (seen.onRemoved.on !== "remove" || seen.onRemoved.at !== "0")
        failures.push(`removed, focus is on ${JSON.stringify(seen.onRemoved)}`)
      if (!seen.editAfter) failures.push("the server left is not editable")
      if (!seen.toggleAfter) failures.push("the server left cannot be switched")
    } finally {
      writeFileSync(stack.config, original)
    }
    seen.dropped = await context.opened.drop()
    seen.restored =
      (await visible(page.locator(css.mcpServersIn("listed")), 20_000)) &&
      (await visible(page.locator(css.mcpEmpty), 5000))
    await settled(page)
    if (!seen.restored) failures.push("the restored list is not shown empty")
    return { seen, failures }
  },

  "non-admin": async (page, stack, context) => {
    const failures = []
    const reader = await signedIn(
      context.browser,
      stack,
      await conversingCredential(stack.client),
      context.layout,
    )
    try {
      await openIntegrations(reader.page)
      const shown = await visible(
        reader.page.locator(css.mcpServersIn("not-admin")),
        15_000,
      )
      // Long enough for a request the tab should not send to have gone.
      await sleep(1500)
      const group = reader.page.locator(css.mcpGroup)
      const tab = reader.page.locator(css.mcpServers)
      const seen = {
        shown,
        phase: await tab.getAttribute("data-mcp-servers").catch(() => null),
        connection: await tab.getAttribute("data-connection").catch(() => null),
        text: (await group.textContent())?.includes(names.mcp.notAdmin),
        controls: await group.locator(css.control).count(),
        requests: reader.sent,
      }
      if (!shown) failures.push("the tab does not show the administrator notice")
      if (!seen.text) failures.push(`no "${names.mcp.notAdmin}"`)
      if (seen.controls !== 0) failures.push(`${seen.controls} controls in the card`)
      // The gateway decides: one list asked, refused, and nothing after it.
      if (JSON.stringify(seen.requests) !== JSON.stringify(["mcpServers.list"]))
        failures.push(
          `requests ${JSON.stringify(seen.requests)}, expected one list alone`,
        )
      return { seen, failures: [...failures, ...reader.errors] }
    } finally {
      await reader.close()
    }
  },
}

/**
 * Asks the agent to call `show_chart` once, allows that call alone, and waits
 * for the turn to end. A gateway with no sign-in for the agent refuses the
 * conversation: "could not run", with what it said.
 */
async function chartTurn(client, conversationId, agent) {
  try {
    await client.conversation.create({ conversationId, agent })
    await client.conversation.send(
      conversationId,
      `Use the tools of the "${SERVER}" MCP server. Call ${CHART_TOOL} (no arguments) ` +
        "exactly once, and wait for its result. Do not use any other tool. " +
        "When it has returned, reply with DONE.",
    )
  } catch (error) {
    throw new CannotRun(
      `the gateway refused the ${agent} conversation (is ${agent} signed in on this machine?): ${error.message}`,
    )
  }
  let admitted = null
  const answered = new Set()
  for (let i = 0; i < 300; i += 1) {
    await sleep(1000)
    const view = await client.conversation.read(conversationId)
    for (;;) {
      const { allow, extra } = admitOnce(view, admitted, answered, SERVER, CHART_TOOL)
      if (extra) throw new CannotRun(`${agent} called ${CHART_TOOL} more than once`)
      if (!allow) break
      admitted = allow.call
      answered.add(permissionKey(allow.permission))
      await client.conversation.answer(
        conversationId,
        allow.permission.executionId,
        allow.permission.permissionId,
        allow.option.id,
      )
    }
    const last = view.messages.at(-1)
    if (last && !["running", "queued"].includes(last.status)) {
      const outcome = setupOutcome(view, SERVER, CHART_TOOL)
      if (outcome.kind === "repeated")
        throw new CannotRun(
          `${agent} called ${CHART_TOOL} more than once (${outcome.calls.map(callKey).join(", ")})`,
        )
      if (outcome.kind !== "ready")
        throw new CannotRun(
          `${agent}'s turn ended ${last.status}${last.error ? ` (${last.error.code ?? last.error})` : ""}; ` +
            `${CHART_TOOL}: ${outcome.call ? outcome.call.status : "not called"}; its reply: ${JSON.stringify(
              last.parts
                .filter((part) => part.kind === "text")
                .map((part) => part.text)
                .join("")
                .slice(0, 400),
            )}`,
        )
      return outcome.call
    }
  }
  throw new CannotRun(`${agent}'s turn did not end within 300 s`)
}

/** The Done-when step in one engine's page: the conversation opened, its chart app drawn once. */
async function chartDrawn(page, title) {
  const failures = []
  const sessionRow = page.getByText(title, { exact: true }).first()
  if (!(await visible(sessionRow, 20_000)))
    throw new CannotRun(`no session row "${title}" in the window`)
  await sessionRow.click()
  await need(page, css.appView, "the app's view in the conversation", 30_000)
  const drawn = await page
    .waitForSelector(css.appFrameIn("inline"), { timeout: 30_000, state: "attached" })
    .then(() => true)
    .catch(() => false)
  if (!drawn) {
    const lifecycle = await page
      .locator(css.appView)
      .first()
      .getAttribute("data-app-view")
    failures.push(`the chart is not drawn inline within 30 s: its view is ${lifecycle}`)
    return { seen: { lifecycle }, failures }
  }
  const { app } = await appFrame(page, "inline", 30_000)
  await app.waitForSelector("#chart", { timeout: 20_000 }).catch(() => {})
  await settled(page)
  const frames = (await page.$$(css.appFrameIn("inline"))).length
  const mounts = oneMount(frames)
  const seen = {
    frames,
    chart: await app.evaluate(
      () => document.querySelector("#chart")?.textContent ?? null,
    ),
  }
  if (mounts) failures.push(mounts)
  if (seen.chart !== "chart for nessa-test")
    failures.push(`the app frame shows "${seen.chart}", not the chart`)
  return { seen, failures }
}

await main(
  meta,
  async ({ options, rep, target: stack }) => {
    const only = chosen(options.only, steps, options.list)
    rep.add({ name: "setup", timings: stack.timings, failures: [] })
    if (options.shots) mkdirSync(options.shots, { recursive: true })
    const layout = options.layouts[0]
    const walk = only.filter((name) => name !== "done-when")
    if (walk.length)
      await withEngines(options, rep, async (engine, browser) => {
        const started = Date.now()
        let opened
        try {
          opened = await signedIn(browser, stack, stack.token(), layout)
          await openIntegrations(opened.page)
        } catch (error) {
          await opened?.close()
          rep.add({
            name: "open",
            engine,
            layout,
            cannotRun: error instanceof CannotRun,
            error: error.message.split("\n")[0],
          })
          return
        }
        const context = { opened, browser, layout }
        let stopped = null
        try {
          for (const name of walk) {
            if (stopped) {
              rep.add({
                name,
                engine,
                layout,
                cannotRun: true,
                error: `not run: ${stopped} failed`,
              })
              continue
            }
            const at = Date.now()
            let result
            try {
              result = await checks[name](opened.page, stack, context)
            } catch (error) {
              result = {
                failures: [],
                cannotRun: error instanceof CannotRun,
                error: error.message.split("\n")[0],
              }
            }
            const entry = rep.add({
              name,
              engine,
              layout,
              ms: Date.now() - at,
              ...result,
              failures: [...(result.failures ?? []), ...opened.errors.splice(0)],
            })
            if (options.shots)
              await opened.page.screenshot({
                path: join(options.shots, `mcp-servers-${engine}-${name}.png`),
              })
            if (!entry.ok) stopped = name
          }
        } finally {
          // A too-large step that stopped leaves its hand-edited file: put
          // back, so the next engine and done-when start from the original.
          if (context.original) writeFileSync(stack.config, context.original)
          await opened.close()
          log(`${engine}: ${Date.now() - started} ms`)
        }
      })
    if (!only.includes("done-when")) return
    // The issue's Done-when: added from the window, called by the agent, drawn.
    let title
    let first = true
    await withEngines(options, rep, async (engine, browser) => {
      const at = Date.now()
      let opened
      try {
        opened = await signedIn(browser, stack, stack.token(), layout)
        const seen = {}
        const setup = []
        if (first) {
          first = false
          await openIntegrations(opened.page)
          await visible(opened.page.locator(css.mcpServersIn("listed")), 20_000)
          await addServer(opened.page)
          if (!(await visible(row(opened.page, SERVER))))
            throw new Error(`${SERVER} was not added from the window`)
          seen.added = await switchOf(opened.page, SERVER)
          if (seen.added.checked !== "true")
            setup.push(`${SERVER}'s switch is ${JSON.stringify(seen.added)}, not on`)
          const conversationId = randomUUID()
          const turnAt = Date.now()
          const call = await chartTurn(stack.client, conversationId, stack.agent)
          seen.agentTurnMs = Date.now() - turnAt
          seen.call = { status: call.status, resourceUri: call.mcp.resourceUri }
          const { conversations } = await stack.client.conversation.list({})
          title = conversations.find(
            (each) => each.conversationId === conversationId,
          )?.title
          if (!title) throw new CannotRun("the conversation has no title to find it by")
          // Reloaded to show the new conversation. Of what the reload
          // reports, only Chromium's aborted `/browser/check` is set aside,
          // as `signedIn` sets aside its load's; every other error is kept.
          const beforeReload = opened.errors.splice(0)
          await opened.page.goto(`${stack.url}?gateway`, {
            waitUntil: "domcontentloaded",
          })
          await need(opened.page, css.anyReady, "the desktop window", 30_000)
          setAsideLoadAbort(opened.errors)
          opened.errors.unshift(...beforeReload)
        }
        if (!title) throw new CannotRun("not run: the conversation was not made")
        const drawn = await chartDrawn(opened.page, title)
        rep.add({
          name: "done-when",
          engine,
          layout,
          ms: Date.now() - at,
          seen: { ...seen, ...drawn.seen },
          failures: [...setup, ...drawn.failures, ...opened.errors.splice(0)],
        })
        if (options.shots)
          await opened.page.screenshot({
            path: join(options.shots, `mcp-servers-${engine}-done-when.png`),
          })
      } catch (error) {
        rep.add({
          name: "done-when",
          engine,
          layout,
          ms: Date.now() - at,
          cannotRun: error instanceof CannotRun,
          error: error.message.split("\n")[0],
          detail: error.message,
        })
      } finally {
        await opened?.close()
      }
    })
  },
  startStack,
)
