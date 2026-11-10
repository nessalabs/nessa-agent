#!/usr/bin/env node
/**
 * A 79-row Agents overview finishes Show All inside the stress check's 10s
 * wait, in Chromium and WebKit (#1249). The drifting light is a scaled
 * texture: a live full-window blur makes each of those frames too long.
 */
import { attempt } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, keys, names } from "./lib/selectors.mjs"
import { seededLoadSpec, withSeeded } from "./lib/seeded-load.mjs"

const sessions = 79
const spec = {
  ...seededLoadSpec,
  sessions,
  longTranscripts: 0,
  messages: 0,
  messageCharacters: 0,
}
const deadline = 10_000

const meta = {
  name: "ambient-overview",
  summary: "79-row Show All finishes within 10s; the drift is not a live blur",
  defaults: { engine: "chromium,webkit", layout: "columns" },
  help: `
Usage: node verification/desktop/scripts/ambient-overview.mjs [options]

Checks, per engine:
  texture    each drifting light is the 192px texture and has no CSS blur
  show-all   the overview's first list, then Show All, each finish within 10s
             and the list then has all ${sessions} rows`,
}

await main(meta, async ({ options, rep, url }) => {
  const seeded = withSeeded(url, spec)
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts) {
      const base = { engine, layout }
      const opened = await openPage(browser, { url: seeded, layout })
      const page = opened.page
      try {
        await attempt(rep, { ...base, name: "texture" }, async () => {
          await page.waitForSelector(".desktop-ambient-light-low", { timeout: 30_000 })
          const light = await page.evaluate(() => {
            const el = document.querySelector(".desktop-ambient-light")
            const style = getComputedStyle(el)
            const root = getComputedStyle(document.querySelector(".desktop-ambient"))
            return {
              filter: style.filter,
              width: style.width,
              scale: root.getPropertyValue("--ambient-scale").trim(),
              count: document.querySelectorAll(".desktop-ambient-light").length,
            }
          })
          const failures = []
          if (light.count !== 2)
            failures.push(`the window has ${light.count} lights, not 2`)
          if (light.filter !== "none")
            failures.push(`the light's filter is ${light.filter}`)
          if (light.width !== "192px")
            failures.push(`the light is ${light.width}, not 192px`)
          if (!(Number(light.scale) > 1))
            failures.push(`the light's scale is ${light.scale}`)
          return { failures }
        })
        await attempt(rep, { ...base, name: "show-all" }, async () => {
          const failures = []
          await page.keyboard.press(keys.overview)
          const openedAt = Date.now()
          const first = await page
            .locator(css.overviewListed)
            .waitFor({ timeout: deadline })
            .then(() => true)
            .catch(() => false)
          const firstMs = Date.now() - openedAt
          if (!first)
            failures.push(`the first list did not finish within 10s (${firstMs}ms)`)
          const show = page.getByRole("button", {
            name: names.overviewShowAll,
            exact: true,
          })
          const marked = Date.now()
          const offered = await show
            .click({ timeout: deadline })
            .then(() => true)
            .catch(() => false)
          const clickMs = Date.now() - marked
          if (!offered)
            failures.push(`Show All was not offered within 10s (${clickMs}ms)`)
          const waited = Date.now()
          const listed = await page
            .locator(css.overviewListed)
            .waitFor({ timeout: deadline })
            .then(() => true)
            .catch(() => false)
          const listedMs = Date.now() - waited
          if (!listed)
            failures.push(`Show All did not finish the list within 10s (${listedMs}ms)`)
          const rows = await page.locator("[data-overview-item]").count()
          if (rows !== sessions)
            failures.push(`the overview lists ${rows} rows, not ${sessions}`)
          return { failures, firstMs, clickMs, listedMs, rows }
        })
      } finally {
        await opened.close()
      }
    }
  })
})
