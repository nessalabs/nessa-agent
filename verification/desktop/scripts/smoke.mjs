#!/usr/bin/env node
/**
 * Smoke: the page loads in each layout, a new session sends, a split opens
 * a pane, an empty draft is not listed, Settings opens, shows Advanced ›
 * Experimental, and closes, the Agents overview is offered on a fresh
 * profile and opens and is left — and no console or page error is raised
 * along the way (the known favicon 404 aside).
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
  ambient-grain    a decoded 160px PNG tile, repeated at 0.06 opacity with overlay blend
  draft-unlisted   ⌘N adds no row to the lists until something is sent
  send             a message typed in a new session appears in its transcript
  split            ⇧⌘N (new session beside) adds a pane
  settings         ⌘, opens Settings; Advanced shows its one tab, Experimental, empty
                   and with no control; its Back button closes it and the panes return
  overview         on a fresh profile the sidebar offers "Agents"; ⌘0 shows the
                   overview and the keyboard lands on its row; Escape returns to the
                   panes (focus.mjs covers Escape before it lands)
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

      await attempt(rep, { ...base, name: "ambient-grain" }, async () => {
        await need(page, css.ambientGrain, "the ambient grain")
        const grain = await page.evaluate(async (selector) => {
          const style = getComputedStyle(document.querySelector(selector))
          const url = /^url\("(.*)"\)$/.exec(style.backgroundImage)?.[1]
          if (!url) return { failures: ["grain has no image URL"] }
          const response = await fetch(url)
          const blob = await response.blob()
          const image = new Image()
          image.src = url
          await image.decode()
          return {
            type: blob.type,
            width: image.naturalWidth,
            height: image.naturalHeight,
            opacity: style.opacity,
            blend: style.mixBlendMode,
            repeat: style.backgroundRepeat,
          }
        }, css.ambientGrain)
        if (grain.failures) return grain
        const failures = []
        if (grain.type !== "image/png")
          failures.push(`grain type ${grain.type}, expected baked image/png (#370)`)
        if (grain.width !== 160 || grain.height !== 160)
          failures.push(`grain tile ${grain.width}×${grain.height}, expected 160×160`)
        if (
          grain.opacity !== "0.06" ||
          grain.blend !== "overlay" ||
          grain.repeat !== "repeat"
        )
          failures.push(`grain layer changed: ${JSON.stringify(grain)}`)
        return { ...grain, failures }
      })

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
        // Advanced › Experimental: the home of previews, empty while none is on offer.
        await page.locator(css.settingsCategory, { hasText: "Advanced" }).click()
        await page
          .locator(css.settingsHeading, { hasText: "Advanced" })
          .waitFor({ state: "visible", timeout: 3000 })
          .catch(() =>
            failures.push("Advanced's page did not open from its sidebar entry"),
          )
        const advanced = await page.evaluate(
          ({ tab, panel, control }) => {
            const page = document.querySelector(panel)
            return {
              tabs: [...document.querySelectorAll(tab)].map((each) => ({
                name: each.textContent?.trim(),
                selected: each.getAttribute("aria-selected") === "true",
              })),
              text: page?.textContent ?? "",
              controls: page?.querySelectorAll(control).length ?? -1,
            }
          },
          { tab: css.settingsTab, panel: css.settingsPanel, control: css.control },
        )
        if (
          advanced.tabs.length !== 1 ||
          advanced.tabs[0].name !== "Experimental" ||
          !advanced.tabs[0].selected
        )
          failures.push(
            `Advanced's tabs are ${JSON.stringify(advanced.tabs)}, expected Experimental alone, selected`,
          )
        if (!advanced.text.includes("Nothing to try right now."))
          failures.push(
            `Experimental's page says "${advanced.text}", not its empty state`,
          )
        if (advanced.controls !== 0)
          failures.push(
            `Experimental's page shows ${advanced.controls} controls, expected none`,
          )
        await leaveSettings(page)
        const gone = await page
          .locator(css.settings)
          .first()
          .waitFor({ state: "hidden", timeout: 3000 })
          .then(() => true)
          .catch(() => false)
        if (!gone) failures.push("Settings still visible after its Back button")
        if (!(await paneCount(page))) failures.push("no panes after leaving Settings")
        return { advanced, failures }
      })

      await attempt(rep, { ...base, name: "overview" }, async () => {
        const failures = []
        // Nothing is stored about the overview (openPage seeds only the layout):
        // the sidebar offers it as it is.
        const entry = page.locator(css.overviewEntry)
        const offered = (await entry.count()) === 1 && (await entry.isVisible())
        if (!offered)
          failures.push(`the sidebar's Agents entry (${css.overviewEntry}) is not shown`)
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
