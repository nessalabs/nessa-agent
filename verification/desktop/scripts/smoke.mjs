#!/usr/bin/env node
/**
 * Smoke: the page loads in each layout, a new session sends, a split opens
 * a pane, an empty draft is not listed, Settings opens and closes, the Agents
 * overview opens and is left — and no console or page error is raised along
 * the way (the known favicon 404 aside).
 */
import { attempt } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { content, css, keys } from "./lib/selectors.mjs"
import {
  contentIs,
  focusComposer,
  leaveSettings,
  paneCount,
  paneCountIs,
  settled,
  state,
  until,
} from "./lib/workspace.mjs"

const meta = {
  name: "smoke",
  summary: "load both layouts, send, split, Settings, overview; no console errors",
  defaults: { engine: "chromium,webkit" },
  help: `
Usage: node verification/desktop/scripts/smoke.mjs [options]

Checks, per engine and layout:
  loads            the page renders panes
  draft-unlisted   ⌘N adds no row to the lists until something is sent
  send             a message typed in a new session appears in its transcript
  split            ⇧⌘N (new session beside) adds a pane
  settings         ⌘, opens Settings; its Back button closes it and the panes return
  overview         ⌘0 shows the Agents overview and the keyboard lands on its row;
                   Escape returns to the panes (focus.mjs covers Escape before it lands)
  console          no console.error / pageerror / failed request (favicon 404 ignored)`,
}

await main(meta, async ({ options, rep, url }) => {
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts) {
      const base = { engine, layout }
      let opened
      const loaded = await attempt(rep, { ...base, name: "loads" }, async () => {
        opened = await openPage(browser, { url, layout })
        await need(opened.page, css.pane, "a pane")
        return { panes: await paneCount(opened.page) }
      })
      if (!loaded.ok) {
        await opened?.close()
        continue
      }
      const { page } = opened
      const rows = () => page.locator(css.sessionRow).count()

      await attempt(rep, { ...base, name: "draft-unlisted" }, async () => {
        const before = await rows()
        await page.keyboard.press(keys.newSession)
        await settled(page)
        const after = await rows()
        return {
          rows: { before, after },
          failures:
            after > before
              ? [`an empty draft was listed (${before} → ${after} rows)`]
              : [],
        }
      })

      await attempt(rep, { ...base, name: "send" }, async () => {
        const words = `smoke ${Date.now().toString(36)}`
        const composer = `${css.focusedPane} ${css.field}`
        await need(page, composer, "the focused pane's composer")
        await page.locator(composer).click()
        await page.keyboard.type(words, { delay: 10 })
        await page.keyboard.press(keys.enter)
        const shown = await until(
          page,
          ([pane, text]) =>
            [...document.querySelectorAll(pane)].some((p) =>
              p.textContent.includes(text),
            ),
          [css.pane, words],
        )
        return {
          failures: shown ? [] : [`"${words}" did not appear in any pane after Enter`],
        }
      })

      await attempt(rep, { ...base, name: "split" }, async () => {
        const before = await paneCount(page)
        await page.keyboard.press(keys.newSessionBeside)
        await paneCountIs(page, before + 1)
        const after = await paneCount(page)
        return {
          panes: { before, after },
          failures:
            after === before + 1
              ? []
              : [`expected ${before + 1} panes after ⇧⌘N, found ${after}`],
        }
      })

      await attempt(rep, { ...base, name: "settings" }, async () => {
        const failures = []
        await page.keyboard.press(keys.settings)
        const shown = await page
          .locator(css.settings)
          .first()
          .waitFor({ state: "visible", timeout: 3000 })
          .then(() => true)
          .catch(() => false)
        if (!shown) failures.push(`Settings (${css.settings}) not visible after ⌘,`)
        await leaveSettings(page)
        const gone = await page
          .locator(css.settings)
          .first()
          .waitFor({ state: "hidden", timeout: 3000 })
          .then(() => true)
          .catch(() => false)
        if (!gone) failures.push("Settings still visible after its Back button")
        if (!(await paneCount(page))) failures.push("no panes after leaving Settings")
        return { failures }
      })

      await attempt(rep, { ...base, name: "overview" }, async () => {
        const failures = []
        await focusComposer(page)
        await page.keyboard.press(keys.overview)
        await contentIs(page, content.overview)
        const open = await state(page)
        if (open.content !== content.overview)
          failures.push(
            `content is ${open.content} after ⌘0, expected ${content.overview}`,
          )
        // Escape leaves whether or not the keyboard has landed yet; waiting
        // for it keeps this check about the overview opening and leaving.
        const landed = await until(
          page,
          (item) => document.activeElement?.closest(item) != null,
          css.overviewItem,
        )
        if (!landed)
          failures.push("the keyboard did not land on an overview row after ⌘0")
        await page.keyboard.press(keys.escape)
        await contentIs(page, content.panes)
        const left = await state(page)
        if (left.content !== content.panes)
          failures.push(
            `content is ${left.content} after Escape, expected ${content.panes}`,
          )
        return { failures }
      })

      rep.add({
        ...base,
        name: "console",
        failures: opened.errors,
        harmless: opened.harmless,
      })
      await opened.close()
    }
  })
})
