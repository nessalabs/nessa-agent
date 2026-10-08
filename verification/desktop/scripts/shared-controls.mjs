#!/usr/bin/env node
/**
 * The window's shared controls (#632): patterns the app used to draw several
 * ways, each now drawn by one component of the kit (`@nessa-ui/react`) and
 * given the window's inks in one rule. Every instance on the page is held to
 * that component's measured contract.
 */
import { openPage, need, withEngines } from "./lib/browser.mjs"
import { attempt, chosen } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css, keys } from "./lib/selectors.mjs"

/**
 * Measures every key cap on the page against the kit's `Kbd` as the window
 * skins it (`chrome.css`): 18px tall, at least as wide, 11px medium type, the
 * window's small corner, a 7% fill and the muted ink. In the page.
 */
function measureKeyCaps(selector) {
  const probe = (property, value) => {
    const element = document.createElement("div")
    element.style.setProperty(property, value)
    document.querySelector("[data-surface]").append(element)
    const style = getComputedStyle(element)
    const read = {
      background: style.backgroundColor,
      color: style.color,
      "border-radius": style.borderTopLeftRadius,
    }[property]
    element.remove()
    return read
  }
  const want = {
    fill: probe("background", "color-mix(in oklab, var(--foreground) 7%, transparent)"),
    ink: probe("color", "var(--desktop-muted)"),
    radius: probe("border-radius", "var(--desktop-radius-xs)"),
  }
  const failures = []
  const caps = [...document.querySelectorAll(selector)].filter((cap) => {
    const box = cap.getBoundingClientRect()
    return (
      box.width > 0 && box.height > 0 && getComputedStyle(cap).visibility !== "hidden"
    )
  })
  const seen = []
  for (const cap of caps) {
    const box = cap.getBoundingClientRect()
    const style = getComputedStyle(cap)
    const name = cap.textContent.trim()
    seen.push({ key: name, width: +box.width.toFixed(2), height: box.height })
    if (cap.dataset.slot !== "kbd") {
      failures.push(`${name}: a <kbd> that is not the kit's Kbd`)
      continue
    }
    if (Math.abs(box.height - 18) > 0.5)
      failures.push(`${name}: ${box.height}px tall, not 18`)
    if (box.width < 17.5) failures.push(`${name}: ${box.width}px wide, under 18`)
    if (style.fontSize !== "11px" || style.fontWeight !== "500")
      failures.push(`${name}: type ${style.fontSize} ${style.fontWeight}, not 11px 500`)
    if (style.backgroundColor !== want.fill)
      failures.push(`${name}: fill ${style.backgroundColor}, not ${want.fill}`)
    if (style.color !== want.ink)
      failures.push(`${name}: ink ${style.color}, not ${want.ink}`)
    if (style.borderTopLeftRadius !== want.radius)
      failures.push(`${name}: corner ${style.borderTopLeftRadius}, not ${want.radius}`)
  }
  return { failures, caps: seen }
}

/**
 * What the window's scale resolves to on this page, read through a probe in
 * a surface: the fills a row answers with and the unread weight. In the page.
 */
function windowScale() {
  const probe = document.createElement("div")
  document.querySelector("[data-surface]").append(probe)
  const read = (value) => {
    probe.style.background = value
    return getComputedStyle(probe).backgroundColor
  }
  probe.style.fontWeight = "var(--desktop-unread-weight)"
  const scale = {
    hover: read("var(--desktop-hover)"),
    press: read("var(--desktop-press)"),
    selected: read("var(--desktop-selected)"),
    unread: getComputedStyle(probe).fontWeight,
  }
  probe.remove()
  return scale
}

/**
 * Every row of the window's lists, by kind, as the two row components draw
 * them: the kit's `SidebarMenuItem` (its control carries `data-size`) or the
 * window's `ListRow` (`.desktop-list-row`). In the page.
 */
function rowsOnPage() {
  const kit = (element) =>
    element.matches('[data-slot="sidebar-menu-item-row"] > [data-size]')
  const listRow = (element) => element.classList.contains("desktop-list-row")
  const label = (element) =>
    element.querySelector(
      '[data-slot="sidebar-menu-item-label"], .desktop-list-row-label',
    )
  const rows = [
    ...document.querySelectorAll(
      [
        ".workspace-sidebar [data-row]",
        ".workspace-sidebar button[aria-current]",
        ".workspace-sidebar .agents-overview-entry",
        ".workspace-list [data-session-row]",
        "[data-overview-item]:not(.agents-request)",
        "#workspace-switcher-results [role='option']",
      ].join(", "),
    ),
  ].filter((row) => row.getBoundingClientRect().height > 0 && !row.dataset.section)
  return rows.map((row) => ({
    text: (label(row)?.textContent ?? row.textContent).trim().slice(0, 40),
    component: kit(row) ? "kit" : listRow(row) ? "list-row" : "other",
    background: getComputedStyle(row).backgroundColor,
    weight: label(row) ? getComputedStyle(label(row)).fontWeight : null,
    unread: row.hasAttribute("data-unread"),
    current:
      row.getAttribute("data-active") === "true" || row.hasAttribute("data-selected"),
  }))
}

/** Rows that are neither of the two components, and current or unread rows off the scale. */
function rowFailures(rows, scale, where) {
  const failures = []
  for (const row of rows) {
    if (row.component === "other")
      failures.push(`${where}: "${row.text}" is neither the kit's row nor ListRow`)
    if (row.unread && row.weight !== scale.unread)
      failures.push(
        `${where}: unread "${row.text}" weighs ${row.weight}, not ${scale.unread}`,
      )
    if (row.current && row.background !== scale.selected)
      failures.push(
        `${where}: chosen "${row.text}" is filled ${row.background}, not ${scale.selected}`,
      )
  }
  return failures
}

/** Points at the first row a selector finds that is neither chosen nor open, and reads its fill. */
async function hovered(page, selector) {
  const row = page
    .locator(selector)
    .filter({ hasNot: page.locator("[data-never]") })
    .and(page.locator(':not([data-active="true"]):not([data-selected]):not([data-open])'))
    .first()
  if (!(await row.count())) return null
  await row.hover()
  // Past the fill's own transition (`--desktop-fast`).
  await page.waitForTimeout(250)
  const fill = await row.evaluate((element) => getComputedStyle(element).backgroundColor)
  await page.mouse.move(1, 1)
  return fill
}

const rowKinds = {
  columns: {
    "sidebar row": ".workspace-sidebar [data-slot='sidebar-menu-item-row'] > [data-size]",
    "pinned session": ".workspace-sidebar [data-row='session']",
    "session list row": ".workspace-list [data-session-row]",
  },
  sidebar: {
    "channel row": ".workspace-sidebar [data-row='channel']",
    "branch session": ".workspace-sidebar [data-row='session']",
  },
}

const checks = {
  keys: async (page) => {
    await need(page, `${css.keyCap}:visible`, "a key cap")
    const onLoad = await page.evaluate(measureKeyCaps, css.keyCap)
    await page.keyboard.press(keys.switcher)
    await need(page, `${css.switcherResults} ${css.keyCap}`, "the switcher's keys")
    await switcherSettled(page)
    const inSwitcher = await page.evaluate(
      measureKeyCaps,
      `${css.switcherResults} ${css.keyCap}`,
    )
    const failures = [...onLoad.failures, ...inSwitcher.failures]
    if (onLoad.caps.length === 0) failures.push("no key cap on load")
    return { failures, measured: { onLoad: onLoad.caps, inSwitcher: inSwitcher.caps } }
  },
  rows: async (page, { layout }) => {
    const scale = await page.evaluate(windowScale)
    const failures = []
    const measured = { scale, hover: {} }
    const onLoad = await page.evaluate(rowsOnPage)
    failures.push(...rowFailures(onLoad, scale, "on load"))
    for (const [kind, selector] of Object.entries(rowKinds[layout] ?? {})) {
      const fill = await hovered(page, selector)
      measured.hover[kind] = fill
      if (fill === null) failures.push(`no ${kind} to point at`)
      else if (fill !== scale.hover)
        failures.push(`a ${kind} under the pointer is ${fill}, not ${scale.hover}`)
    }
    await page.keyboard.press(keys.overview)
    await need(page, css.overviewRow, "an overview row")
    await page.waitForTimeout(400)
    const inOverview = await page.evaluate(rowsOnPage)
    failures.push(...rowFailures(inOverview, scale, "in the overview"))
    const finished = inOverview.filter((row) => row.unread)
    if (finished.length === 0) failures.push("no finished row in the overview to weigh")
    const overviewFill = await hovered(page, `${css.overviewRow}:not(:focus)`)
    measured.hover["overview row"] = overviewFill
    if (overviewFill !== scale.hover)
      failures.push(
        `an overview row under the pointer is ${overviewFill}, not ${scale.hover}`,
      )
    await page.keyboard.press(keys.switcher)
    await need(page, css.switcherResults, "the switcher")
    await switcherSettled(page)
    const inSwitcher = await page.evaluate(rowsOnPage)
    failures.push(...rowFailures(inSwitcher, scale, "in the switcher"))
    if (!inSwitcher.some((row) => row.current))
      failures.push("no chosen row in the switcher")
    measured.rows = onLoad.length + inOverview.length + inSwitcher.length
    return { failures, measured }
  },
}

/** Waits for the switcher to finish scaling in (a running glyph spins forever, so only its own). */
function switcherSettled(page) {
  return page.evaluate((selector) => {
    const switcher = document.querySelector(selector)?.closest('[role="dialog"]')
    return Promise.all(
      (switcher?.getAnimations({ subtree: true }) ?? [])
        .filter((animation) => animation.effect?.getTiming().iterations !== Infinity)
        .map((animation) => animation.finished.catch(() => {})),
    )
  }, css.switcherResults)
}

await main(
  {
    name: "shared-controls",
    summary: "the window's repeated patterns: one component each, measured",
    defaults: { engine: "chromium,webkit" },
    options: {
      only: { type: "string" },
    },
    help: `
Checks, per engine and layout (--only keys,rows):
  keys       every <kbd> in the window (the sidebar's search, the session list's
             search, the quick switcher's rows) is the kit's Kbd: 18px tall and at
             least as wide, 11px medium type, --desktop-radius-xs, a 7% fill and
             --desktop-muted ink. Measured on load and with the switcher open.
  rows       every row of the window's lists — the sidebar's, the session list's,
             the overview's sessions and the switcher's — is the kit's
             SidebarMenuItem or ListRow; under the pointer each kind fills with
             --desktop-hover, a chosen row with --desktop-selected, and an unread
             title weighs --desktop-unread-weight. On load, in the overview and
             with the switcher open.

Settings is left out (#632 › Settings is redesigned on its own branch).`,
  },
  async ({ options, rep, url }) => {
    const names = chosen(options.only, Object.keys(checks), options.list)
    await withEngines(options, rep, async (engine, browser) => {
      for (const layout of options.layouts)
        for (const name of names)
          await attempt(rep, { name, engine, layout }, async () => {
            const { page, close } = await openPage(browser, { url, layout })
            try {
              await need(page, css.anyReady, "the window")
              return await checks[name](page, { layout })
            } finally {
              await close().catch(() => {})
            }
          })
    })
  },
)
