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
 * the tab: add the test MCP server (`scripts/mcp-test-server/server.mjs`) with
 * one variable, inspect it, turn it off, rename it, measure it narrow, meet a
 * conflict a Node client makes first, remove it; then a credential that may
 * only converse sees the administrator notice and sends no mcpServers
 * request. Last, the issue's Done-when: the server added again from the
 * window, a conversation in which the agent calls `show_chart`, and the
 * chart's app drawn once in the window.
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
import { mkdirSync, readFileSync } from "node:fs"
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

const steps = [
  "empty",
  "add",
  "inspect",
  "toggle",
  "rename",
  "narrow",
  "conflict",
  "remove",
  "non-admin",
  "done-when",
]

const meta = {
  name: "mcp-servers-gateway",
  summary:
    "Settings › Integrations over a real gateway: add, inspect, toggle, rename, conflict, remove, non-admin, and an app drawn from a server added there",
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
  add        ${SERVER} added with one variable: one row, its switch on,
             "1 variable", and the variable's value nowhere in the page
  inspect    show_chart has a UI badge, ${DESTRUCTIVE} is destructive, the
             answer is complete; Inspect rests while it runs
  toggle     the switch turns it off: one save, the switch resting in flight,
             and off as the new list says
  rename     renamed to ${RENAMED}: one row, its variable kept
  narrow     with the row, the form and the inspection open, at 800 and 390px:
             nothing outside its card, no sideways scroll, the fold held, the
             row's actions under its text under a 420px page
  conflict   a Node client saves first; the window's save is refused, says so,
             reloads, and keeps what was typed
  remove     asked first, then removed: the row gone, "No servers yet"
  non-admin  a credential that may only converse (server.read and
             conversation.*) sees the administrator notice,
             no control, and sends no mcpServers request
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
    return { url: dev.url, mode: "dev", close, client, agent, timings, token }
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
 * A page signed in with `token` through `/browser/login` from the page's own
 * origin, on the gateway's window, counting each mcpServers request it sends
 * on its socket (`sent`).
 */
async function signedIn(browser, stack, token, layout) {
  const opened = await openPage(browser, { url: stack.url, layout })
  const { page } = opened
  const sent = []
  page.on("websocket", (socket) =>
    socket.on("framesent", ({ payload }) => {
      try {
        const frame = JSON.parse(
          typeof payload === "string" ? payload : payload.toString(),
        )
        if (typeof frame.method === "string" && frame.method.startsWith("mcpServers."))
          sent.push(frame.method)
      } catch {
        // Not a JSON frame: not a request, so not one counted.
      }
    }),
  )
  const status = await page.evaluate(
    async (token) =>
      (
        await fetch("/browser/login", {
          method: "POST",
          credentials: "same-origin",
          headers: { "Content-Type": "application/json", "X-Nessa-Browser": "1" },
          body: JSON.stringify({ token }),
        })
      ).status,
    token,
  )
  if (status < 200 || status > 299) {
    await opened.close()
    throw new CannotRun(`the gateway refused the page's sign-in (${status})`)
  }
  await page.goto(`${stack.url}?gateway`, { waitUntil: "domcontentloaded" })
  await need(page, css.anyReady, "the desktop window", 30_000)
  opened.errors.splice(0)
  return { ...opened, sent }
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

/** Fills the add form and saves it; `secret` as one variable's value, when given. */
async function addServer(page, secret) {
  await button(page.locator(css.mcpGroup), names.mcp.add).click()
  const add = form(page)
  await add.getByLabel(names.mcp.name, { exact: true }).fill(SERVER)
  await add.getByLabel(names.mcp.command, { exact: true }).fill(process.execPath)
  await add.getByLabel(names.mcp.args, { exact: true }).fill(serverScript)
  if (secret !== undefined) {
    await button(add, names.mcp.addVariable).click()
    const variable = add.locator(css.mcpVariable).last()
    await variable.getByLabel(names.mcp.variableName, { exact: true }).fill(VARIABLE)
    await variable.locator('input[type="password"]').fill(secret)
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
    await addServer(page, context.secret)
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
    const running = await page.locator(css.mcpInspectionIn("running")).count()
    const restingWhileRunning =
      running > 0 ? await button(row(page, SERVER), names.mcp.inspect).isDisabled() : null
    const done = await visible(page.locator(css.mcpInspectionIn("done")), 45_000)
    const panel = page.locator(css.mcpInspection)
    const seen = {
      ms: Date.now() - at,
      phase: await panel.getAttribute("data-mcp-inspection"),
      restingWhileRunning,
      chartUi: await panel
        .locator(`${css.mcpTool(CHART_TOOL)} ${css.mcpBadge("ui")}`)
        .count(),
      destructive: await panel
        .locator(`${css.mcpTool(DESTRUCTIVE)} ${css.mcpBadge("destructive")}`)
        .count(),
      cut: await panel.locator(css.mcpCut).count(),
      tools: await panel.locator("[data-mcp-tool]").count(),
    }
    if (!done) failures.push(`the inspection is ${seen.phase}, not done within 45 s`)
    if (restingWhileRunning === false) failures.push("Inspect is enabled while running")
    if (seen.chartUi !== 1) failures.push(`${CHART_TOOL} has ${seen.chartUi} UI badges`)
    if (seen.destructive !== 1) failures.push(`${DESTRUCTIVE} is not marked destructive`)
    if (seen.cut !== 0) failures.push("the inspection says it is incomplete")
    await button(panel, names.mcp.close).click()
    if (!(await gone(panel))) failures.push("the inspection did not close")
    return { seen, failures }
  },

  toggle: async (page, stack, context) => {
    const failures = []
    const before = context.opened.sent.length
    const toggle = row(page, SERVER).locator(css.mcpSwitch)
    // Whether the switch rested at any moment while its save was in flight.
    await toggle.evaluate((element) => {
      window.__mcpSwitchRested = false
      new MutationObserver(() => {
        if (element.disabled) window.__mcpSwitchRested = true
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
      requests: context.opened.sent.slice(before),
    }
    if (!off) failures.push(`the switch is ${JSON.stringify(seen.switch)}, not off`)
    if (!seen.restedInFlight)
      failures.push("the switch did not rest while its save was in flight")
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
    const placeholder = await edit
      .locator('input[type="password"]')
      .getAttribute("placeholder")
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
    if (placeholder !== "Stored value kept")
      failures.push(`the stored value's placeholder is "${placeholder}"`)
    if (!closed) failures.push("the form is still open after the save")
    if (JSON.stringify(seen.rows) !== JSON.stringify([RENAMED]))
      failures.push(`rows ${JSON.stringify(seen.rows)}, expected [${RENAMED}]`)
    if (seen.variables !== "1 variable") failures.push(`the row says "${seen.variables}"`)
    if (seen.secretInPage) failures.push("the variable's value is in the page")
    return { seen, failures }
  },

  narrow: async (page) => {
    // The row, the form and the inspection all open, then measured.
    await button(row(page, RENAMED), names.mcp.inspect).click()
    await visible(page.locator(css.mcpInspectionIn("done")), 45_000)
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
    const args = edit.getByLabel(names.mcp.args, { exact: true })
    await args.fill(`${serverScript}\n--typed`)
    await button(edit, names.mcp.save).click()
    const said = await waitFor(
      async () => (await page.locator(css.mcpNotice).allTextContents()).join(" "),
      10_000,
    )
    await waitFor(async () => (await switchOf(page, RENAMED)).checked === "true", 5000)
    const seen = {
      notice: said,
      formOpen: (await form(page).count()) === 1,
      kept: await args.inputValue().catch(() => null),
      switch: await switchOf(page, RENAMED),
      requests: context.opened.sent.slice(before),
    }
    if (!seen.notice.includes(names.mcp.conflict))
      failures.push(`the window says "${seen.notice}", not the conflict`)
    if (!seen.formOpen) failures.push("the form closed on the conflict")
    if (seen.kept !== `${serverScript}\n--typed`)
      failures.push("what was typed was not kept")
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
    const before = context.opened.sent.length
    await button(row(page, RENAMED), names.mcp.remove).click()
    const asked = await row(page, RENAMED).locator(css.mcpConfirm).textContent()
    const sentOnAsk = context.opened.sent.length - before
    await button(row(page, RENAMED), names.mcp.remove).click()
    const removed = await gone(row(page, RENAMED))
    const empty = await visible(page.locator(css.mcpEmpty))
    const seen = {
      asked,
      sentOnAsk,
      removed,
      empty,
      requests: context.opened.sent.slice(before),
    }
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
        requests: reader.sent.length,
      }
      if (!shown) failures.push("the tab does not show the administrator notice")
      if (!seen.text) failures.push(`no "${names.mcp.notAdmin}"`)
      if (seen.controls !== 0) failures.push(`${seen.controls} controls in the card`)
      if (seen.requests !== 0) failures.push(`${seen.requests} mcpServers requests sent`)
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
        if (first) {
          first = false
          await openIntegrations(opened.page)
          await visible(opened.page.locator(css.mcpServersIn("listed")), 20_000)
          await addServer(opened.page)
          if (!(await visible(row(opened.page, SERVER))))
            throw new Error(`${SERVER} was not added from the window`)
          seen.added = await switchOf(opened.page, SERVER)
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
          // Reloaded to show the new conversation. What the reload's own
          // load reports before the window is ready is set aside, as
          // `signedIn` sets aside its load's (Chromium reports each
          // `/browser/check` the load sends as aborted); everything before
          // the reload, and after the window is ready, is kept.
          const beforeReload = opened.errors.splice(0)
          await opened.page.goto(`${stack.url}?gateway`, {
            waitUntil: "domcontentloaded",
          })
          await need(opened.page, css.anyReady, "the desktop window", 30_000)
          opened.errors.splice(0, Infinity, ...beforeReload)
        }
        if (!title) throw new CannotRun("not run: the conversation was not made")
        const drawn = await chartDrawn(opened.page, title)
        rep.add({
          name: "done-when",
          engine,
          layout,
          ms: Date.now() - at,
          seen: { ...seen, ...drawn.seen },
          failures: [...drawn.failures, ...opened.errors.splice(0)],
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
