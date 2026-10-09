#!/usr/bin/env node
/**
 * Settings' page, as a person meets it (src/desktop/settings/ui): its
 * masthead clear of the window's controls and the common pages whole above
 * the fold; the bar's small title once the masthead has scrolled away,
 * itself clear of the controls; a page arriving quickly, or at once under
 * less motion; search landing on a setting and showing it; ⌘F; and the
 * window's focus ring on its controls. Measured as numbers, in Chromium and
 * WebKit.
 */
import { attempt, chosen } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, keys, safeAreaTokens } from "./lib/selectors.mjs"
import { frames, settled } from "./lib/workspace.mjs"

/** Sizes a person keeps a Mac window at, and one narrow enough to fold the sidebar. */
const sizes = [
  { width: 1440, height: 900 },
  { width: 1000, height: 700 },
  { width: 760, height: 700 },
  { width: 600, height: 700 },
]

/**
 * The pages people open most, whose settings must all be in view without
 * scrolling at 1000 × 700: each category's first tab, but Connections and
 * Advanced (a list of agents, and nothing yet).
 */
const aboveTheFold = [
  "General",
  "Appearance",
  "Workspace",
  "Models",
  "Privacy & Permissions",
]

/** The longest page, which scrolls at every size: Workspace › Keyboard. */
const longPage = { category: "Workspace", tab: "Keyboard" }

/** Opens Settings and waits for it to have arrived. */
async function openSettings(page) {
  await page.keyboard.press(keys.settings)
  await need(page, css.settings, "Settings")
  await settled(page)
}

/** Shows a category (and a tab of it), revealing a folded sidebar for the click. */
async function show(page, category, tab) {
  const folded = await page.$eval(css.settings, (s) => s.dataset.sidebar === "closed")
  if (folded) await page.keyboard.press(keys.toggleSidebar)
  await page.locator(css.settingsCategory, { hasText: category }).first().click()
  if (folded) await page.keyboard.press(keys.toggleSidebar)
  if (tab) await page.locator(css.settingsTab, { hasText: tab }).first().click()
  await settled(page)
}

/** The window controls' corner, resolved on Settings' own surface. */
function corner(page) {
  return page.evaluate(
    ({ settings, tokens }) => {
      const surface = document.querySelector(settings)
      const px = (token) => {
        const probe = document.createElement("div")
        probe.style.cssText = `position:absolute;visibility:hidden;width:var(${token})`
        surface.append(probe)
        const width = probe.getBoundingClientRect().width
        probe.remove()
        return width
      }
      return { safe: px(tokens.start), bar: px(tokens.height) }
    },
    { settings: css.settings, tokens: safeAreaTokens },
  )
}

const checks = {
  /**
   * At every size, each category's title sits below the titlebar row (so
   * never under the window's controls), with room above it; at 1000 × 700
   * the common pages' rows are all above the fold.
   */
  masthead: async (page) => {
    const failures = []
    const seen = []
    const categories = await page.$$eval(css.settingsCategory, (items) =>
      items.map((item) => item.textContent.trim()),
    )
    for (const size of sizes) {
      await page.setViewportSize(size)
      await frames(page, 2)
      await settled(page)
      const { bar } = await corner(page)
      for (const category of categories) {
        await show(page, category)
        const m = await page.evaluate(
          (sel) => {
            const title = document.querySelector(sel.heading).getBoundingClientRect()
            const rows = [...document.querySelectorAll(`${sel.panel} ${sel.row}`)].map(
              (row) => Math.round(row.getBoundingClientRect().bottom),
            )
            return {
              top: Math.round(title.top),
              left: Math.round(title.left),
              right: Math.round(title.right),
              lastRow: rows.length ? Math.max(...rows) : null,
              sidebar: document.querySelector(sel.settings).dataset.sidebar,
            }
          },
          {
            heading: css.settingsHeading,
            panel: css.settingsPanel,
            row: css.settingsRow,
            settings: css.settings,
          },
        )
        seen.push({ size: `${size.width}x${size.height}`, category, ...m })
        // Breathing room: a title starts at least 16px below the controls' row.
        if (m.top < bar + 16)
          failures.push(
            `${size.width}x${size.height} ${category}: title at y=${m.top}, within 16px of the titlebar row (${Math.round(bar)})`,
          )
        if (m.left < 0 || m.right > size.width)
          failures.push(
            `${size.width}x${size.height} ${category}: title outside the window`,
          )
        if (
          size.width === 1000 &&
          aboveTheFold.includes(category) &&
          m.lastRow !== null &&
          m.lastRow > size.height
        )
          failures.push(
            `1000x700 ${category}: a row ends at y=${m.lastRow}, below the fold`,
          )
      }
    }
    return { seen, failures }
  },

  /**
   * Scrolled, the bar shows the page's name, starting with the column — and
   * after the window's controls with the sidebar folded; scrolled back, it
   * goes.
   */
  condense: async (page) => {
    const failures = []
    const seen = []
    for (const size of [sizes[1], sizes[3]]) {
      await page.setViewportSize(size)
      await frames(page, 2)
      await show(page, longPage.category, longPage.tab)
      const { safe } = await corner(page)
      const scroll = (top) =>
        page.$eval(css.settingsScroll, (el, y) => el.scrollTo({ top: y }), top)
      const read = () =>
        page.evaluate(
          (sel) => {
            const title = document.querySelector(sel.barTitle)
            const box = title.getBoundingClientRect()
            return {
              condensed: document
                .querySelector(sel.content)
                .hasAttribute("data-condensed"),
              opacity: Number(getComputedStyle(title).opacity),
              left: Math.round(box.left),
              text: title.textContent,
              sidebar: document.querySelector(sel.settings).dataset.sidebar,
              column: Math.round(
                document.querySelector(sel.heading).getBoundingClientRect().left,
              ),
            }
          },
          {
            barTitle: css.settingsBarTitle,
            content: css.settingsContent,
            settings: css.settings,
            heading: css.settingsHeading,
          },
        )
      // From the top: a click may have scrolled the page to bring its tab into view.
      await scroll(0)
      await frames(page, 2)
      await settled(page)
      const before = await read()
      await scroll(400)
      await page.waitForFunction(
        (sel) => document.querySelector(sel).hasAttribute("data-condensed"),
        css.settingsContent,
        { timeout: 3000 },
      )
      await settled(page)
      const after = await read()
      await scroll(0)
      await page.waitForFunction(
        (sel) => !document.querySelector(sel).hasAttribute("data-condensed"),
        css.settingsContent,
        { timeout: 3000 },
      )
      await settled(page)
      const back = await read()
      const at = `${size.width}x${size.height}`
      seen.push({ size: at, before, after, back })
      if (before.opacity !== 0)
        failures.push(`${at}: the bar's title showed before scrolling`)
      if (after.opacity !== 1)
        failures.push(`${at}: the bar's title did not show, scrolled`)
      if (after.text !== longPage.category)
        failures.push(`${at}: the bar named "${after.text}", not ${longPage.category}`)
      if (after.sidebar === "closed" && after.left < safe)
        failures.push(
          `${at}: the bar's title starts at x=${after.left}, under the controls (${Math.round(safe)})`,
        )
      if (after.sidebar === "open" && Math.abs(after.left - after.column) > 1)
        failures.push(
          `${at}: the bar's title starts at x=${after.left}, not with the column (${after.column})`,
        )
      if (back.opacity !== 0)
        failures.push(`${at}: the bar's title stayed, scrolled back`)
    }
    return { seen, failures }
  },

  /**
   * A page arrives quickly — its motion at most 300ms, on the page and its
   * masthead only — and at once, with nothing moving, under less motion.
   */
  arrive: async (page, { browser, url }) => {
    const failures = []
    const measure = async (target) => {
      await show(target, "Models")
      await target.locator(css.settingsCategory, { hasText: "Workspace" }).first().click()
      return target.evaluate(
        (sel) =>
          [document.querySelector(sel.masthead), document.querySelector(sel.panel)]
            .flatMap((el) => (el ? el.getAnimations() : []))
            .map((a) => ({
              name: a.animationName,
              duration: Number(a.effect.getTiming().duration),
            })),
        { masthead: css.settingsMasthead, panel: ".settings-panel" },
      )
    }
    const normal = await measure(page)
    if (normal.length === 0) failures.push("no arrival motion with motion allowed")
    for (const each of normal)
      if (each.duration > 300)
        failures.push(`${each.name} runs ${each.duration}ms, over 300ms`)
    const reduced = await openPage(browser, {
      url,
      width: 1000,
      height: 700,
      reducedMotion: "reduce",
    })
    let still
    try {
      await openSettings(reduced.page)
      still = await measure(reduced.page)
      if (still.length > 0)
        failures.push(`under less motion a page still moves: ${JSON.stringify(still)}`)
    } finally {
      await reduced.close()
    }
    return { normal, reduced: still, failures }
  },

  /**
   * Search: Enter takes the first result's page, shows the setting in the
   * middle of the view and marks it a moment; ⌘F comes back to the field.
   */
  search: async (page) => {
    const failures = []
    await page.setViewportSize(sizes[1])
    await show(page, "General")
    const field = page.locator(css.settingsSearch)
    await field.fill("drifting light")
    await page.keyboard.press("Enter")
    const landed = await page
      .waitForSelector(css.settingsFound, { timeout: 3000 })
      .then(() => true)
      .catch(() => false)
    if (!landed) failures.push("no setting was marked where the search landed")
    await settled(page)
    const m = await page.evaluate(
      (sel) => {
        const row = document.querySelector('[data-setting="drifting-light"]')
        const scroller = document.querySelector(sel.scroll).getBoundingClientRect()
        const box = row?.getBoundingClientRect()
        return {
          heading: document.querySelector(sel.heading)?.textContent,
          tab: document.querySelector(`${sel.tab}[aria-selected="true"]`)?.innerText,
          inView: Boolean(
            box && box.top >= scroller.top && box.bottom <= scroller.bottom,
          ),
        }
      },
      { scroll: css.settingsScroll, heading: css.settingsHeading, tab: css.settingsTab },
    )
    if (m.heading !== "Appearance")
      failures.push(`search took ${m.heading}, not Appearance`)
    if (m.tab !== "Motion") failures.push(`search took the ${m.tab} tab, not Motion`)
    if (!m.inView) failures.push("the setting search landed on is not in view")
    // The mark fades by itself.
    const faded = await page
      .waitForSelector(css.settingsFound, { state: "detached", timeout: 3000 })
      .then(() => true)
      .catch(() => false)
    if (!faded) failures.push("the mark where the search landed stayed")
    await page.locator(css.settingsHeading).click()
    await page.keyboard.press("Meta+KeyF")
    const focused = await page.evaluate(
      (sel) => document.activeElement?.matches(sel) ?? false,
      css.settingsSearch,
    )
    if (!focused) failures.push("⌘F did not go to Settings' search")
    await field.fill("")
    return { landed: m, failures }
  },

  /** Keyboard focus on a tab and a switch shows the window's focus ring. */
  "focus-ring": async (page, { engine }) => {
    const failures = []
    // WebKit on a Mac moves Tab through fields only, as Safari does; ⌥Tab
    // goes through every control, as a person with full keyboard access does.
    const tab = engine === "webkit" ? "Alt+Tab" : "Tab"
    await page.setViewportSize(sizes[1])
    await show(page, "Appearance", "Theme")
    const ring = (selector) =>
      page.evaluate((sel) => {
        const el = document.activeElement
        if (!el?.matches(sel))
          return { focused: `${el?.tagName} ${el?.getAttribute("role")}` }
        const style = getComputedStyle(el)
        return { style: style.outlineStyle, width: parseFloat(style.outlineWidth) }
      }, selector)
    // From Theme, → walks to Header (and shows it), as a keyboard does.
    await page.locator(css.settingsTab, { hasText: "Theme" }).first().focus()
    await page.keyboard.press("ArrowRight")
    await need(page, '[data-setting="tint-from-picture"]', "Appearance › Header")
    await settled(page)
    const onTab = await ring(css.settingsTab)
    // Then Tab goes into the page, to its first switch.
    await page.keyboard.press(tab)
    const control = await ring('[role="switch"]')
    for (const [what, got] of [
      ["tab", onTab],
      ["switch", control],
    ])
      if (!got?.style || got.style === "none" || !(got.width >= 1.5))
        failures.push(`the ${what} took focus without the ring: ${JSON.stringify(got)}`)
    return { tab: onTab, control, failures }
  },
}

const meta = {
  name: "settings-page",
  summary: "Settings' page: masthead, condensing bar, arrival motion, search, focus",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/settings-page.mjs [options]

  --only <list>   Checks, comma-separated: masthead, condense, arrive, search, focus-ring

Per engine:
  masthead    every title below the titlebar row (16px clear) at 1440×900,
              1000×700, 760×700 and 600×700; common pages whole above the
              fold at 1000×700
  condense    scrolled, the bar names the page, with the column or after
              the window's controls; scrolled back, it goes
  arrive      a page's motion is at most 300ms; none under less motion
  search      Enter lands on the setting, in view and marked a moment; ⌘F
  focus-ring  a tab and a switch show the window's focus ring`,
}

await main(meta, async ({ options, rep, url }) => {
  const names = chosen(options.only, Object.keys(checks), options.list)
  await withEngines(options, rep, async (engine, browser) => {
    for (const name of names)
      await attempt(rep, { engine, name }, async () => {
        const opened = await openPage(browser, { url, width: 1000, height: 700 })
        try {
          await openSettings(opened.page)
          return await checks[name](opened.page, { browser, url, engine })
        } finally {
          await opened.close()
        }
      })
  })
})
