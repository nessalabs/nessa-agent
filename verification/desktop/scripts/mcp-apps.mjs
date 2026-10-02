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
  escape-navigate, escape-refresh, escape-rewrite
               the app sending its frame away, by script, by meta refresh, or
               after rewriting its document (which erases its reporter):
               refused (nothing reaches the other site), the frame taken off
               the page, and the host says it cannot show the app
  departures   the real proxy, handed documents the host's own builder writes
               (dev server only): every way a frame's document is replaced —
               a rewrite then a reload, a navigation or about:blank; a
               rewrite alone; a navigation or reload before the first load; a
               document with no reporter, or one answering the check without
               the frame's token — is the app's departure, said once
               and nothing relayed after it; an app left alone, forging
               departures with no token or a wrong one, or a third party
               forging them, is not
  forge        forged proxy messages change nothing; a forged report puts no
               words of the app's in the host's chrome
  teardown     closing the pane takes its frames off the page; the app asking to
               go is sent ui/resource-teardown before its pane closes`,
}

/** Says hello to the host, as an app would, each time a document of its runs. */
const hello = `parent.postMessage({ jsonrpc: "2.0", method: "hello-from-app" }, "*");`

/**
 * Documents for the \`departures\` check, by name: the HTML handed to the
 * proxy (behind the host's own policy and reporter, unless \`bare\`), and
 * whether the proxy must report the app gone.
 */
const departureScenarios = {
  "left-alone": { html: `<p>quiet</p><script>${hello}</script>`, leaves: false },
  "forged-departures": {
    html: `<script>${hello}setTimeout(function () {
      parent.postMessage({ jsonrpc: "2.0", method: "ui/notifications/sandbox-app-left", params: {} }, "*");
      parent.postMessage({ jsonrpc: "2.0", method: "ui/notifications/sandbox-app-left", params: { token: "__nessa_departure_token__" } }, "*");
      parent.postMessage({ jsonrpc: "2.0", method: "ui/notifications/sandbox-app-left", params: { token: "0123456789abcdef0123456789abcdef" } }, "*");
    }, 300)</script>`,
    leaves: false,
  },
  "rewrite-alone": {
    html: `<script>${hello}setTimeout(function () { document.open(); document.write("<p>rewritten</p>"); document.close(); }, 300)</script>`,
    leaves: true,
  },
  "rewrite-then-reload": {
    html: `<script>${hello}setTimeout(function () { document.open(); document.write("<p>w</p>"); document.close(); setTimeout(function () { location.reload(); }, 200); }, 300)</script>`,
    leaves: true,
  },
  "rewrite-then-navigate": {
    html: `<script>${hello}setTimeout(function () { document.open(); document.write("<p>w</p>"); document.close(); setTimeout(function () { location = "https://example.com/?leak=rewrite"; }, 200); }, 300)</script>`,
    leaves: true,
  },
  "rewrite-then-blank": {
    html: `<script>${hello}setTimeout(function () { document.open(); setTimeout(function () { location = "about:blank"; }, 200); }, 300)</script>`,
    leaves: true,
  },
  "navigate-before-load": {
    html: `<script>${hello}location.replace("about:blank")</script>`,
    leaves: true,
  },
  "reload-before-load": {
    html: `<script>${hello}if (!window.name) { window.name = "reloaded"; location.reload(); }</script>`,
    leaves: true,
  },
  // What the app posts as its document goes reaches the proxy with no
  // sender (the frame's window is gone): after its reporter's word, never
  // relayed.
  "posts-as-it-goes": {
    html: `<script>${hello}addEventListener("pagehide", function () { ${hello} });
      setTimeout(function () { location = "about:blank"; }, 300)</script>`,
    leaves: true,
  },
  "no-reporter": {
    html: `<p>a document the host did not write</p>`,
    bare: true,
    leaves: true,
  },
  // A document the host did not write that knows the protocol: it answers
  // the check, but has no token to answer with. (It cannot spell the slot:
  // the proxy fills the first, which the host puts in its reporter, and
  // \`forged-departures\` holds an app that spells it behind the reporter.)
  impostor: {
    html: `<script>addEventListener("message", function (event) {
      if (event.data && event.data.method === "ui/notifications/sandbox-app-check") {
        parent.postMessage({ jsonrpc: "2.0", method: "ui/notifications/sandbox-app-check", params: {} }, "*");
        parent.postMessage({ jsonrpc: "2.0", method: "ui/notifications/sandbox-app-check", params: { token: "0123456789abcdef0123456789abcdef" } }, "*");
      }
    })</script>`,
    bare: true,
    leaves: true,
  },
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
  for (const more of await page.getByText(names.showAll).all()) {
    if (await row.count()) break
    await more.click()
  }
  if (!(await row.count())) throw new CannotRun(`no session row "${names.appSession}"`)
  await row.click()
  await need(page, css.appFrameIn("inline"), "the app's card frame", 10_000)
  await settled(page)
  return opened
}

/** The app's own document in the frame drawn in `place`: the proxy's one child. */
async function appFrame(page, place, timeout = 10_000) {
  const until = Date.now() + timeout
  while (Date.now() < until) {
    const element = await page.$(css.appFrameIn(place))
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
    await frame.waitForSelector(css.fixtureState(state), { timeout })
  } catch {
    // Reported below.
  }
  return frame.evaluate(
    (says) =>
      Object.fromEntries(
        Object.entries(says).map(([field, attribute]) => [
          field,
          document.body.getAttribute(attribute),
        ]),
      ),
    names.fixtureSays,
  )
}

/** What an output of the fixture app says, once it says anything but `pending`. */
async function output(frame, id, pending = "", timeout = 5000) {
  await frame
    .waitForFunction(
      ([id, pending]) => {
        const text = document.querySelector(id)?.textContent ?? ""
        return text !== "" && text !== pending
      },
      [id, pending],
      { timeout },
    )
    .catch(() => {})
  return frame.evaluate((id) => document.querySelector(id)?.textContent ?? "", id)
}

const rect = (element) =>
  element.evaluate((e) => {
    const r = e.getBoundingClientRect()
    return { x: r.left, y: r.top, w: r.width, h: r.height }
  })

/**
 * The place's body and the app's frame in it, measured together once the
 * place has settled (a pane opening animates its width): whether the frame
 * fills the body, within a pixel.
 */
async function fills(page, body, frame) {
  const measure = () =>
    page.evaluate(
      ([body, frame]) => {
        const r = (sel) => {
          const box = document.querySelector(sel)?.getBoundingClientRect()
          return box && { x: box.left, y: box.top, w: box.width, h: box.height }
        }
        return { body: r(body), frame: r(frame) }
      },
      [body, frame],
    )
  await until(
    page,
    ([body, frame]) => {
      const a = document.querySelector(body)?.getBoundingClientRect()
      const b = document.querySelector(frame)?.getBoundingClientRect()
      return (
        !!a &&
        !!b &&
        Math.abs(a.width - b.width) <= 1 &&
        Math.abs(a.height - b.height) <= 1
      )
    },
    [body, frame],
  ).catch(() => {})
  const both = await measure()
  const fit =
    !!both.body &&
    !!both.frame &&
    Math.abs(both.body.w - both.frame.w) <= 1 &&
    Math.abs(both.body.h - both.frame.h) <= 1
  return { ...both, fit }
}

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
  const answer = await output(app, css.fixtureOutput("mode"))
  if (answer !== 'ok: {"mode":"inline"}')
    failures.push(`the card's fullscreen request was answered ${answer}, expected inline`)
  return expectLive(page, "pane", "fullscreen", failures)
}

const checks = {
  inline: async (page) => {
    const failures = []
    const { app, box, said } = await expectLive(page, "inline", "inline", failures)
    // The host context it was told: the page's theme and its tokens.
    const told = JSON.parse(said.context ?? "{}")
    const theme = await page.evaluate(() =>
      document.documentElement.classList.contains("dark") ? "dark" : "light",
    )
    if (told.theme !== theme)
      failures.push(`the app was told theme ${told.theme}, not ${theme}`)
    if (!told.styles?.variables?.["--color-text-primary"])
      failures.push(`the app was told no text colour: ${JSON.stringify(told.styles)}`)
    const height = await app.evaluate(() => document.documentElement.scrollHeight)
    await until(
      page,
      ([frame, want]) =>
        Math.abs(document.querySelector(frame).getBoundingClientRect().height - want) <=
        1,
      [css.appFrameIn("inline"), height],
    ).catch(() => {})
    const drawn = await rect(await page.$(css.appFrameIn("inline")))
    if (Math.abs(drawn.h - height) > 1)
      failures.push(`the card's frame is ${drawn.h}px tall; the app said ${height}px`)
    return { frame: box, drawn, appHeight: height, told: told.styles, failures }
  },

  pane: async (page) => {
    const failures = []
    const { box } = await openPane(page, failures)
    const card = await appState((await appFrame(page, "inline")).app, "live")
    if (card.mode !== "inline") failures.push(`the card's app is told ${card.mode} after`)
    const measured = await fills(
      page,
      `${css.widgetPane} ${css.widgetBody}`,
      `${css.widgetPane} ${css.appFrameIn("pane")}`,
    )
    if (!measured.fit)
      failures.push(
        `the pane's frame does not fill its body: ${JSON.stringify(measured)}`,
      )
    return { frame: box, measured, failures }
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
    const measured = await fills(
      page,
      `${css.widgetWindow} ${css.widgetBody}`,
      css.appFrameIn("window"),
    )
    if (!measured.fit)
      failures.push(
        `the window's frame does not fill its body: ${JSON.stringify(measured)}`,
      )
    return { frame: box, measured, failures }
  },

  "tools-call": async (page) => {
    const failures = []
    const { app } = await appFrame(page, "inline")
    await appState(app, "live")
    await app.click(css.fixtureControl("call-allowed"))
    const allowed = await output(app, css.fixtureOutput("call"))
    if (
      allowed !==
      'ok: {"content":[{"type":"text","text":"Refreshed"}],"structuredContent":{"refreshed":{"times":1}}}'
    )
      failures.push(`the allowed tool was answered ${allowed}`)
    await app.click(css.fixtureControl("call-hidden"))
    const hidden = await output(app, css.fixtureOutput("call"), allowed)
    if (hidden !== `error: ${names.hiddenToolRefused}`)
      failures.push(`the hidden tool was answered ${hidden}`)
    return { allowed, hidden, failures }
  },

  csp: async (page) => {
    const failures = []
    const { app } = await appFrame(page, "inline")
    await appState(app, "live")
    await app.click(css.fixtureControl("fetch"))
    const fetched = await output(app, css.fixtureOutput("fetch"), "fetching")
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

  ...Object.fromEntries(
    ["navigate", "refresh", "rewrite"].map((control) => [
      `escape-${control}`,
      async (page) => {
        const failures = []
        const leaked = []
        page.on("request", (request) => {
          if (new URL(request.url()).hostname === "example.com")
            leaked.push(request.url())
        })
        const { app } = await appFrame(page, "inline")
        await appState(app, "live")
        // The app's frame sent away: refused by the proxy's policy, and the
        // frame, which is no longer the app's, taken off the page — said.
        await app.click(css.fixtureControl(control))
        const line = page.locator(css.appView, { hasText: names.appLoadLine })
        await line
          .first()
          .waitFor({ timeout: 5000 })
          .catch(() => {})
        if (!(await line.count()))
          failures.push(`after ${control}, the host does not say "${names.appLoadLine}"`)
        if (await page.$(css.appFrameIn("inline")))
          failures.push(`after ${control}, the app's frame is still on the page`)
        if (leaked.length > 0)
          failures.push(`requests reached example.com: ${leaked.join(", ")}`)
        return { leaked, failures }
      },
    ]),
  ),

  departures: async (page) => {
    const failures = []
    const leaked = []
    page.on("request", (request) => {
      if (new URL(request.url()).hostname === "example.com") leaked.push(request.url())
    })
    const seen = await page
      .evaluate(
        async ({ scenarios, wait }) => {
          let csp, methods, origin
          try {
            csp = await import("/src/desktop/widgets/app/model/csp.ts")
            methods = (await import("/src/desktop/widgets/app/model/sandbox-methods.ts"))
              .sandboxMethods
            origin =
              await import("/src/desktop/widgets/app/adapters/dom/sandbox-origin.ts")
          } catch {
            return null
          }
          const sandbox = origin.pageSandbox(document)
          if (!sandbox) return null
          const applied = csp.appliedCsp({})
          const policy = csp.cspPolicy(applied)
          const seen = {}
          let handed = 0
          let lastHanded = 0
          for (const [name, { html, bare }] of Object.entries(scenarios)) {
            const proxy = document.createElement("iframe")
            proxy.setAttribute("sandbox", "allow-scripts allow-same-origin")
            proxy.style.cssText = "position:fixed;left:-9999px;width:200px;height:100px"
            proxy.src = sandbox.url
            seen[name] = []
            addEventListener("message", (event) => {
              if (event.source !== proxy.contentWindow) return
              const method = event.data?.method
              if (method === methods.proxyReady) {
                proxy.contentWindow.postMessage(
                  {
                    jsonrpc: "2.0",
                    method: methods.resourceReady,
                    params: {
                      html: bare ? html : csp.appDocument(html, applied),
                      policy,
                    },
                  },
                  sandbox.origin,
                )
                handed += 1
                lastHanded = performance.now()
                return
              }
              seen[name].push(method ?? "?")
            })
            document.body.append(proxy)
          }
          // A third party: a frame of the page's own, on no origin, that
          // forges departures and check answers at every proxy and app.
          const third = document.createElement("iframe")
          third.setAttribute("sandbox", "allow-scripts")
          third.style.cssText = "position:fixed;left:-9999px"
          third.srcdoc = `<script>setTimeout(function () {
            for (var i = 0; i < top.frames.length; i++) {
              var w = top.frames[i];
              if (w === window) continue;
              var forged = [
                { jsonrpc: "2.0", method: ${JSON.stringify(methods.appLeft)}, params: { token: "00000000000000000000000000000000" } },
                { jsonrpc: "2.0", method: ${JSON.stringify(methods.appLeft)}, params: {} },
                { jsonrpc: "2.0", method: ${JSON.stringify(methods.appCheck)}, params: { token: "00000000000000000000000000000000" } }
              ];
              var app = null;
              try { app = w.frames[0] || null; } catch (error) { app = null; }
              forged.forEach(function (m) { w.postMessage(m, "*"); if (app) app.postMessage(m, "*"); });
            }
            parent.postMessage({ forgedAt: top.frames.length - 1 }, "*");
          }, 1500)</script>`
          let forgedAt = 0
          addEventListener("message", (event) => {
            if (event.source === third.contentWindow) forgedAt = event.data?.forgedAt ?? 0
          })
          document.body.append(third)
          // Every document handed over, then past the proxy's check deadline.
          const count = Object.keys(scenarios).length
          const started = performance.now()
          while (handed < count && performance.now() - started < 20_000)
            await new Promise((resolve) => setTimeout(resolve, 100))
          await new Promise((resolve) =>
            setTimeout(resolve, Math.max(0, lastHanded + wait - performance.now())),
          )
          return { seen, appLeft: methods.appLeft, forgedAt, handed }
        },
        { scenarios: departureScenarios, wait: 7000 },
      )
      .catch((error) => {
        throw new CannotRun(`the departures harness failed: ${error.message}`)
      })
    if (seen === null)
      throw new CannotRun("departures needs the dev server's modules and its sandbox")
    if (seen.handed < Object.keys(departureScenarios).length)
      throw new CannotRun(`only ${seen.handed} proxies were ready`)
    // The third party forged at every frame of the page but itself: the
    // proxies, and the window's own.
    if (seen.forgedAt < Object.keys(departureScenarios).length)
      failures.push(`the third party forged at ${seen.forgedAt} frames`)
    for (const [name, { leaves }] of Object.entries(departureScenarios)) {
      const said = seen.seen[name]
      const at = said.indexOf(seen.appLeft)
      if (!leaves && at !== -1) failures.push(`${name}: departed (${said.join(" ")})`)
      if (leaves && at === -1) failures.push(`${name}: no departure (${said.join(" ")})`)
      if (at !== -1 && said.lastIndexOf(seen.appLeft) !== at)
        failures.push(`${name}: departed more than once`)
      if (at !== -1 && said.slice(at + 1).includes("hello-from-app"))
        failures.push(
          `${name}: the app was relayed after its departure (${said.join(" ")})`,
        )
    }
    if (leaked.length > 0)
      failures.push(`requests reached example.com: ${leaked.join(", ")}`)
    return { seen: seen.seen, forgedAt: seen.forgedAt, leaked, failures }
  },

  forge: async (page) => {
    const failures = []
    const { app } = await appFrame(page, "inline")
    await appState(app, "live")
    // Speaking as the proxy, and putting words in the host's chrome.
    await app.click(css.fixtureControl("forge"))
    await page.waitForTimeout(500)
    const view = await page.locator(css.appView).first().getAttribute("data-app-view")
    if (view !== "live")
      failures.push(`after the forged proxy messages the app is ${view}`)
    const said = await appState(app, "live", 1000)
    if (said.state !== "live") failures.push(`the app is ${said.state} after forging`)
    const notices = await page.locator(css.appNotice).allInnerTexts()
    for (const notice of notices)
      if (!names.appNotices.some((line) => line.test(notice)))
        failures.push(`a notice that is not the host's own reached its chrome: ${notice}`)
    return { notices, failures }
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
      css.appFrameIn("pane"),
    ).catch(() => {})
    if (await page.$(css.appFrameIn("pane")))
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
