#!/usr/bin/env node
/**
 * MCP Apps (ADR 344, #349), through the fixture app the sample workspace
 * registers (`src/desktop/widgets/app/fixture/`): it renders inline, in a pane
 * (its own fullscreen request) and in the window, each place told its
 * display mode and given its tool call; `tools/call` is answered for an
 * allowed tool and refused for a hidden one; a request to an origin its CSP
 * does not declare is blocked, and the host says so; the app runs on an
 * opaque origin that reaches neither the window nor storage; and it is torn
 * down on close — by its pane's close, and by its own request.
 *
 * The app's documents are cross-origin to the page, so the script reads them
 * through Playwright's frames, never through the page.
 *
 * Every check runs on a fresh page, in each engine and layout.
 */
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { content, css, names } from "./lib/selectors.mjs"
import { contentIs, paneCount, paneCountIs, settled, until } from "./lib/workspace.mjs"

const meta = {
  name: "mcp-apps",
  summary: "MCP Apps: places, tools/call, CSP, isolation, teardown",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/mcp-apps.mjs [options]

Checks, per engine and layout (--only <names> to pick):
  inline       the card's app is live, told inline, its call's input and result,
               and its frame as tall as the app said
  pane         the app's fullscreen request opens a pane beside the conversation,
               whose app is live and told fullscreen; the card's stays inline
  window       the pane's Open in Window shows the app in the window, live,
               told fullscreen, filling it
  tools-call   tools/call answered for the allowed tool, refused for the hidden one
  csp          a fetch to an undeclared origin is blocked, and the host's notice
               names it
  isolation    the app's origin is opaque: no parent document, no storage; the
               proxy is on another origin than the window
  teardown     closing the pane takes its frames off the page; the app asking to
               go is sent ui/resource-teardown before its pane closes`,
}

/** A fresh page on the sample session whose conversation carries the fixture app. */
async function onSample(browser, { url, layout }) {
  const opened = await openPage(browser, { url, layout })
  const { page } = opened
  // By the session list, not the switcher's chord: what is checked is the
  // app, on any host's keyboard.
  await page.getByText(names.widgetChannel, { exact: true }).first().click()
  const row = page.getByText(names.appSession, { exact: true }).first()
  await row.waitFor({ timeout: 3000 }).catch(() => {})
  // The sidebar lists a channel's latest few; the rest are behind Show all.
  for (const more of await page.getByText(/^Show all \d+$/).all()) {
    if (await row.count()) break
    await more.click()
  }
  if (!(await row.count())) throw new CannotRun(`no session row "${names.appSession}"`)
  await row.click()
  await need(
    page,
    `${css.appFrame}[data-app-frame="inline"]`,
    "the app's card frame",
    10_000,
  )
  await settled(page)
  return opened
}

/** The app's own document in the frame drawn in `place`: the proxy's one child. */
async function appFrame(page, place, timeout = 10_000) {
  const until = Date.now() + timeout
  while (Date.now() < until) {
    const element = await page.$(`${css.appFrame}[data-app-frame="${place}"]`)
    const proxy = await element?.contentFrame()
    const app = proxy?.childFrames()[0]
    if (app && !app.isDetached()) return { element, proxy, app }
    await page.waitForTimeout(100)
  }
  throw new CannotRun(`no app document in the ${place} frame`)
}

/** Waits until the app in `frame` says it is `state` (`data-fixture-state`), and says what it is. */
async function appState(frame, state, timeout = 10_000) {
  try {
    await frame.waitForSelector(`body[data-fixture-state="${state}"]`, { timeout })
  } catch {
    // Reported below.
  }
  return frame.evaluate(() => ({
    state: document.body.getAttribute("data-fixture-state"),
    mode: document.body.getAttribute("data-fixture-mode"),
    input: document.body.getAttribute("data-fixture-input"),
    result: document.body.getAttribute("data-fixture-result"),
  }))
}

/** What an output of the fixture app says, once it says anything but `pending`. */
async function output(frame, id, pending = "", timeout = 5000) {
  await frame
    .waitForFunction(
      ([id, pending]) => {
        const text = document.getElementById(id)?.textContent ?? ""
        return text !== "" && text !== pending
      },
      [id, pending],
      { timeout },
    )
    .catch(() => {})
  return frame.evaluate((id) => document.getElementById(id)?.textContent ?? "", id)
}

const rect = (element) =>
  element.evaluate((e) => {
    const r = e.getBoundingClientRect()
    return { x: r.left, y: r.top, w: r.width, h: r.height }
  })

/** Checks the app in `place` is live, told `mode`, with its call's input and result. */
async function expectLive(page, place, mode, failures) {
  const { element, proxy, app } = await appFrame(page, place)
  const said = await appState(app, "live")
  if (said.state !== "live") failures.push(`the ${place} app is ${said.state}, not live`)
  if (said.mode !== mode)
    failures.push(`the ${place} app is told ${said.mode}, not ${mode}`)
  if (said.input !== '{"title":"Fixture"}')
    failures.push(`the ${place} app was told input ${said.input}`)
  if (said.result !== '{"rows":3}')
    failures.push(`the ${place} app was told result ${said.result}`)
  const box = await rect(element)
  if (box.w <= 0 || box.h <= 0)
    failures.push(`the ${place} frame has no room: ${JSON.stringify(box)}`)
  return { element, proxy, app, said, box }
}

/** Opens the app in a pane, by its own fullscreen request from the card. */
async function openPane(page, failures) {
  const before = await paneCount(page)
  const { app } = await appFrame(page, "inline")
  await appState(app, "live")
  await app.click(css.fixtureControl("fullscreen"))
  await paneCountIs(page, before + 1)
  const answer = await output(app, "mode")
  if (answer !== 'ok: {"mode":"inline"}')
    failures.push(`the card's fullscreen request was answered ${answer}, expected inline`)
  return expectLive(page, "pane", "fullscreen", failures)
}

const checks = {
  inline: async (page) => {
    const failures = []
    const { app, box } = await expectLive(page, "inline", "inline", failures)
    const height = await app.evaluate(() => document.documentElement.scrollHeight)
    await until(
      page,
      ([frame, want]) =>
        Math.abs(document.querySelector(frame).getBoundingClientRect().height - want) <=
        1,
      [`${css.appFrame}[data-app-frame="inline"]`, height],
    ).catch(() => {})
    const drawn = await rect(await page.$(`${css.appFrame}[data-app-frame="inline"]`))
    if (Math.abs(drawn.h - height) > 1)
      failures.push(`the card's frame is ${drawn.h}px tall; the app said ${height}px`)
    return { frame: box, drawn, appHeight: height, failures }
  },

  pane: async (page) => {
    const failures = []
    const { box } = await openPane(page, failures)
    const card = await appState((await appFrame(page, "inline")).app, "live")
    if (card.mode !== "inline") failures.push(`the card's app is told ${card.mode} after`)
    const body = await rect(await page.$(`${css.widgetPane} ${css.widgetBody}`))
    if (Math.abs(body.w - box.w) > 1 || Math.abs(body.h - box.h) > 1)
      failures.push(
        `the pane's frame does not fill its body: ${JSON.stringify({ body, frame: box })}`,
      )
    return { frame: box, body, failures }
  },

  window: async (page) => {
    const failures = []
    await openPane(page, failures)
    await page.locator(css.widgetPane).first().hover()
    await page
      .locator(css.widgetPane)
      .first()
      .getByRole("button", { name: names.openInWindow })
      .first()
      .click()
    await contentIs(page, content.widget)
    const { box } = await expectLive(page, "window", "fullscreen", failures)
    const body = await rect(await page.$(`${css.widgetWindow} ${css.widgetBody}`))
    if (Math.abs(body.w - box.w) > 1 || Math.abs(body.h - box.h) > 1)
      failures.push(
        `the window's frame does not fill its body: ${JSON.stringify({ body, frame: box })}`,
      )
    return { frame: box, body, failures }
  },

  "tools-call": async (page) => {
    const failures = []
    const { app } = await appFrame(page, "inline")
    await appState(app, "live")
    await app.click(css.fixtureControl("call-allowed"))
    const allowed = await output(app, "call")
    if (
      allowed !==
      'ok: {"content":[{"type":"text","text":"Refreshed"}],"structuredContent":{"refreshed":{"times":1}}}'
    )
      failures.push(`the allowed tool was answered ${allowed}`)
    await app.click(css.fixtureControl("call-hidden"))
    const hidden = await output(app, "call", allowed)
    if (hidden !== `error: ${names.hiddenToolRefused}`)
      failures.push(`the hidden tool was answered ${hidden}`)
    return { allowed, hidden, failures }
  },

  csp: async (page) => {
    const failures = []
    const { app } = await appFrame(page, "inline")
    await appState(app, "live")
    await app.click(css.fixtureControl("fetch"))
    const fetched = await output(app, "fetch", "fetching")
    if (fetched !== "blocked") failures.push(`the undeclared fetch was ${fetched}`)
    const notice = page.locator(css.appNotice, { hasText: names.blockedNotice })
    await notice
      .first()
      .waitFor({ timeout: 5000 })
      .catch(() => {})
    if (!(await notice.count()))
      failures.push(
        `no notice "${names.blockedNotice}"; notices: ${JSON.stringify(await page.locator(css.appNotice).allInnerTexts())}`,
      )
    return { fetched, failures }
  },

  isolation: async (page) => {
    const failures = []
    const { proxy, app } = await appFrame(page, "inline")
    await appState(app, "live")
    const inside = await app.evaluate(() => {
      const tries = (run) => {
        try {
          run()
          return "allowed"
        } catch {
          return "refused"
        }
      }
      return {
        origin: window.origin,
        parentDocument: tries(() => window.parent.document.body),
        topDocument: tries(() => window.top.document.body),
        localStorage: tries(() => window.localStorage.getItem("x")),
        cookie: tries(() => document.cookie),
      }
    })
    const pageOrigin = await page.evaluate(() => window.origin)
    const proxyOrigin = await proxy.evaluate(() => window.origin)
    if (inside.origin !== "null")
      failures.push(`the app's origin is ${inside.origin}, not opaque`)
    for (const reach of ["parentDocument", "topDocument", "localStorage", "cookie"])
      if (inside[reach] !== "refused")
        failures.push(`the app's ${reach} was ${inside[reach]}`)
    if (proxyOrigin === pageOrigin || proxyOrigin === "null")
      failures.push(
        `the proxy's origin ${proxyOrigin} is not another origin than ${pageOrigin}`,
      )
    return { inside, pageOrigin, proxyOrigin, failures }
  },

  teardown: async (page) => {
    const failures = []
    // By its pane's close: the frames go with the place.
    const { proxy, app } = await openPane(page, failures)
    const pane = page.locator(css.widgetPane).first()
    await pane.hover()
    await pane.getByRole("button", { name: names.closePane }).first().click()
    await settled(page)
    await until(
      page,
      (frame) => !document.querySelector(frame),
      `${css.appFrame}[data-app-frame="pane"]`,
    ).catch(() => {})
    if (await page.$(`${css.appFrame}[data-app-frame="pane"]`))
      failures.push("the pane's frame is still on the page after its close")
    if (!proxy.isDetached() || !app.isDetached())
      failures.push("the pane's proxy or app document outlived its close")
    // By its own request: teardown is sent, answered, and the pane closes.
    const second = await openPane(page, failures)
    const panes = await paneCount(page)
    // The fixture answers its teardown 400ms after it is asked: until then
    // the pane stays, and the app is still on the page saying so.
    await second.app.click(css.fixtureControl("close"))
    const said = await appState(second.app, "tearing-down", 2000).catch(() => ({
      state: "detached",
    }))
    const waiting = await paneCount(page)
    await paneCountIs(page, panes - 1)
    if (said.state !== "tearing-down")
      failures.push(
        `the app asking to go was not sent ui/resource-teardown (it says ${said.state})`,
      )
    if (waiting !== panes)
      failures.push("the app's pane closed before the app answered its teardown")
    if ((await paneCount(page)) !== panes - 1)
      failures.push("the app's pane did not close after its teardown")
    if (!second.app.isDetached())
      failures.push("the app's document outlived its teardown")
    return { said: said.state, failures }
  },
}

await main(meta, async ({ options, rep, url }) => {
  const only = options.only ? options.list(options.only) : Object.keys(checks)
  for (const name of only)
    if (!Object.hasOwn(checks, name)) throw new CannotRun(`no check named ${name}`)
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts)
      for (const name of only) {
        let opened
        await attempt(rep, { engine, layout, name }, async () => {
          opened = await onSample(browser, { url, layout })
          const result = await checks[name](opened.page, layout)
          return { ...result, failures: [...result.failures, ...opened.errors] }
        }).finally(() => opened?.close())
      }
  })
})
