#!/usr/bin/env node
/**
 * A tool disclosure keeps the clicked header where it is and opens downward.
 * At the end of the transcript, a later growth still follows.
 *
 * The rule is `watchToolDisclosure` (`src/desktop/workspace/ui/transcript/tool-disclosure.ts`).
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"

const meta = {
  name: "tool-disclosure",
  summary:
    "opening a tool keeps its header still and opens downward; the end still follows",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/tool-disclosure.mjs [options] [--shots <dir>]

Checks, per engine and layout (--only <names> to pick):
  group-end    at the end, opening and closing "Ran 3 tools" leaves that
               header's top within 1px, and the open grows the distance
               from the end
  group-away   scrolled to the top, the same header stays within 1px
  step-end     at the end, opening a call's row leaves that row's top
               within 1px
  earlier      opening the folded calls above a long run leaves that
               row's top within 1px
  follow       after a disclosure, a growth while the reader is at the
               end leaves the distance from the end under 24px`,
}

const tolerance = 1

async function openSession(page, id, channel) {
  let row = page.locator(`[data-session-row="${id}"]`)
  if ((await row.count()) === 0 && channel) {
    const branch = page.locator(`[data-channel="${channel}"]`)
    if ((await branch.count()) > 0) await branch.first().click()
    else await page.getByRole("button", { name: new RegExp(`^${channel}\\b`) }).click()
    row = page.locator(`[data-session-row="${id}"]`)
  }
  if ((await row.count()) === 0) throw new CannotRun(`no session ${id}`)
  await row.click()
  await need(page, css.transcriptWorkToggle, "a tool run")
}

/**
 * Press `selector` inside the transcript and sample the header's top for
 * the whole open, including the motion. `up` is how far that top moved up.
 */
function press(page, selector) {
  return page.evaluate(
    ({ selector, transcript }) =>
      new Promise((resolve) => {
        const scroller = document.querySelector(transcript)
        const header = scroller?.querySelector(selector)
        if (!scroller || !header) {
          resolve({ missing: true })
          return
        }
        const before = header.getBoundingClientRect().top
        const scrollBefore = scroller.scrollTop
        const view = scroller.getBoundingClientRect()
        const visible = before >= view.top - 1 && before <= view.bottom - 1
        const distanceBefore =
          scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight
        header.click()
        const tops = []
        const anchors = []
        const start = performance.now()
        const step = () => {
          tops.push(header.getBoundingClientRect().top - before)
          anchors.push(getComputedStyle(scroller).overflowAnchor)
          const held = scroller.hasAttribute("data-disclosure-hold")
          const moving = [...scroller.querySelectorAll("[data-slot='tool-trace']")].some(
            (trace) =>
              trace
                .getAnimations()
                .some((animation) => animation.playState === "running"),
          )
          const elapsed = performance.now() - start
          // At least two frames, then until the hold and its motion are done.
          if ((tops.length < 2 || held || moving) && elapsed < 1000)
            requestAnimationFrame(step)
          else {
            const down = Math.max(0, ...tops)
            const scrollDelta = scroller.scrollTop - scrollBefore
            resolve({
              missing: false,
              before,
              visible,
              distanceBefore,
              distance:
                scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight,
              // Up is the bug. Down is the browser clamping a shrink past the end.
              up: Math.max(0, ...tops.map((top) => -top)),
              down,
              scrollDelta,
              anchors: [...new Set(anchors)],
              held,
              expanded: header.getAttribute("aria-expanded"),
            })
          }
        }
        requestAnimationFrame(step)
      }),
    { selector, transcript: css.transcript },
  )
}

function settle(page) {
  return page.waitForFunction((transcript) => {
    const scroller = document.querySelector(transcript)
    const moving = [
      ...(scroller?.querySelectorAll("[data-slot='tool-trace']") ?? []),
    ].some((trace) =>
      trace.getAnimations().some((animation) => animation.playState === "running"),
    )
    return Boolean(scroller) && !scroller.hasAttribute("data-disclosure-hold") && !moving
  }, css.transcript)
}

async function pin(page, atEnd) {
  await page.evaluate(
    ({ atEnd, transcript }) => {
      const scroller = document.querySelector(transcript)
      scroller.scrollTop = atEnd ? scroller.scrollHeight : 0
    },
    { atEnd, transcript: css.transcript },
  )
  await page.evaluate(
    () =>
      new Promise((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(resolve)),
      ),
  )
}

function moved(sample, failures, name) {
  if (sample.missing) {
    failures.push(`${name} has no header`)
    return
  }
  if (sample.up > tolerance)
    failures.push(
      `${name} moved up ${sample.up.toFixed(1)}px (anchors ${sample.anchors.join(", ")})`,
    )
  // A shrink can only pull the header down by the clamp, which lowers scrollTop.
  if (sample.down > tolerance && sample.scrollDelta > -sample.down + tolerance)
    failures.push(
      `${name} moved down ${sample.down.toFixed(1)}px while scroll moved ${sample.scrollDelta.toFixed(1)}px`,
    )
  if (sample.held) failures.push(`${name} was still holding the scroll after its motion`)
  if (!sample.anchors.includes("none"))
    failures.push(`${name} never turned anchoring off (${sample.anchors.join(", ")})`)
}

const checks = {
  async "group-end"({ page, engine, layout, options }) {
    await openSession(page, "split-panes", "desktop-app")
    await pin(page, true)
    const line = page.locator(css.transcriptWorkToggle).first()
    if ((await line.getAttribute("aria-expanded")) === "true") {
      await line.evaluate((element) => element.click())
      await settle(page)
    }
    await pin(page, true)
    const opened = await press(page, '[data-slot="tool-trace-line"]')
    const failures = []
    if (!opened.visible)
      failures.push(`the run's header is off screen at the end (${opened.before})`)
    moved(opened, failures, "opening the run")
    const waterfall = await page.evaluate(() => {
      const trace = document.querySelector(".workspace-work")
      return {
        axis: trace?.querySelector("[data-slot='tool-trace-axis']")?.textContent ?? "",
        bars: trace?.querySelectorAll("[data-slot='tool-trace-bar']").length ?? 0,
        mini: trace?.querySelector("[data-slot='tool-trace-mini-bar']") != null,
      }
    })
    if (!waterfall.mini) failures.push("the run's header has no bar")
    if (waterfall.bars < 3)
      failures.push(`the run draws ${waterfall.bars} duration bars, not one per call`)
    if (!waterfall.axis.includes("0s") || !waterfall.axis.includes("30s"))
      failures.push(`the time axis is ${JSON.stringify(waterfall.axis)}`)
    if (!(opened.distance > opened.distanceBefore + 8))
      failures.push(
        `opening the run left the end ${opened.distance}px away, from ${opened.distanceBefore}`,
      )
    if (options.shots && engine) {
      mkdirSync(options.shots, { recursive: true })
      await page
        .locator(`${css.message}[data-role="agent"]`)
        .first()
        .screenshot({
          path: join(options.shots, `waterfall-open-${engine}-${layout}.png`),
        })
    }
    const closed = await press(page, '[data-slot="tool-trace-line"]')
    moved(closed, failures, "closing the run")
    return {
      failures,
      opened: opened.up,
      closed: closed.up,
      distance: opened.distance,
    }
  },
  async "group-away"({ page }) {
    await openSession(page, "split-panes", "desktop-app")
    const line = page.locator(css.transcriptWorkToggle).first()
    if ((await line.getAttribute("aria-expanded")) === "true") {
      await line.evaluate((element) => element.click())
      await settle(page)
    }
    await pin(page, false)
    const opened = await press(page, '[data-slot="tool-trace-line"]')
    const failures = []
    if (!opened.visible) failures.push("the run's header is off screen at the top")
    moved(opened, failures, "opening the run away from the end")
    return { failures, moved: opened.up }
  },
  async "step-end"({ page, engine, layout, options }) {
    await openSession(page, "split-panes", "desktop-app")
    const line = page.locator(css.transcriptWorkToggle).first()
    if ((await line.getAttribute("aria-expanded")) !== "true") {
      await line.evaluate((element) => element.click())
      await settle(page)
    }
    await pin(page, true)
    const opened = await press(page, '[data-slot="tool-trace-step"]')
    const failures = []
    if (!opened.visible)
      failures.push(`the call's row is off screen at the end (${opened.before})`)
    moved(opened, failures, "opening the call")
    const card = await page.evaluate(() => {
      const row = document.querySelector("[data-slot='tool-trace-step']")
      const details = row?.getAttribute("aria-controls")
      const open = document.getElementById(details ?? "")
      return {
        expanded: row?.getAttribute("aria-expanded") === "true",
        hasDetails: open != null && open.getBoundingClientRect().height > 20,
      }
    })
    if (!card.expanded || !card.hasDetails)
      failures.push("the call's details did not open under its row")
    if (options.shots && engine && layout === "columns") {
      mkdirSync(options.shots, { recursive: true })
      await page
        .locator(`${css.message}[data-role="agent"]`)
        .first()
        .screenshot({
          path: join(options.shots, `waterfall-call-${engine}.png`),
        })
    }
    if (!(opened.distance > opened.distanceBefore + 8))
      failures.push(
        `opening the call left the end ${opened.distance}px away, from ${opened.distanceBefore}`,
      )
    return { failures, moved: opened.up }
  },
  async earlier({ page }) {
    await openSession(page, "retry-budget", "gateway")
    const lines = page.locator(css.transcriptWorkToggle)
    const count = await lines.count()
    let index = -1
    for (let i = 0; i < count; i++) {
      const text = await lines.nth(i).innerText()
      const ran = /Ran (\d+) tools/.exec(text)
      if (ran && Number(ran[1]) > 4) index = i
    }
    if (index < 0) throw new CannotRun("no run long enough to fold earlier calls")
    const line = lines.nth(index)
    if ((await line.getAttribute("aria-expanded")) !== "true") {
      await line.evaluate((element) => element.click())
      await settle(page)
    }
    await need(page, css.transcriptWorkEarlier, "the folded calls")
    await page.evaluate((selector) => {
      const scroller = document.querySelector(".workspace-transcript")
      const header = scroller.querySelector(selector)
      scroller.scrollTop = Math.max(0, header.offsetTop - 48)
    }, '[data-slot="tool-trace-earlier"]')
    const opened = await press(page, '[data-slot="tool-trace-earlier"]')
    const failures = []
    if (!opened.visible) failures.push("the earlier row is off screen")
    moved(opened, failures, "showing earlier calls")
    return { failures, moved: opened.up }
  },
  async follow({ page }) {
    await openSession(page, "split-panes", "desktop-app")
    await pin(page, true)
    await press(page, '[data-slot="tool-trace-line"]')
    await pin(page, true)
    const followed = await page.evaluate(
      (transcript) =>
        new Promise((resolve) => {
          const scroller = document.querySelector(transcript)
          const inner = scroller.querySelector(".workspace-transcript-inner")
          const node = document.createElement("div")
          node.dataset.followProbe = ""
          node.style.height = "120px"
          inner.append(node)
          requestAnimationFrame(() =>
            requestAnimationFrame(() => {
              resolve({
                grew: node.getBoundingClientRect().height,
                distance:
                  scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight,
                held: scroller.hasAttribute("data-disclosure-hold"),
              })
              node.remove()
            }),
          )
        }),
      css.transcript,
    )
    const failures = []
    if (followed.held) failures.push("the disclosure was still holding during the growth")
    if (followed.grew < 100) failures.push(`the probe grew ${followed.grew}px`)
    if (followed.distance >= 24)
      failures.push(`the end was ${followed.distance}px away after the growth`)
    return { failures, distance: followed.distance }
  },
}

await main(meta, async ({ options, rep, url }) => {
  const only = options.only ? options.only.split(",") : null
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts) {
      const opened = await openPage(browser, {
        url,
        layout,
        width: 1280,
        height: 720,
        dsf: 1,
      })
      try {
        for (const [name, check] of Object.entries(checks)) {
          if (only && !only.includes(name)) continue
          await attempt(rep, { name, engine, layout }, () =>
            check({ page: opened.page, engine, layout, options }),
          )
        }
      } finally {
        await opened.close()
      }
    }
  })
})
