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

/**
 * The window's name at a sidebar's foot (`ui/identity.tsx`): its pill, its
 * words and its row, against the corner controls' scale. In the page.
 */
function measureIdentity() {
  const button = [...document.querySelectorAll(".desktop-identity-button")].find(
    (candidate) => candidate.getClientRects().length > 0 && candidate.checkVisibility(),
  )
  if (!button) return null
  const probe = document.createElement("div")
  document.querySelector("[data-surface]").append(probe)
  probe.style.position = "absolute"
  probe.style.width = "var(--desktop-control-size)"
  probe.style.borderRadius = "var(--desktop-control-radius)"
  probe.style.color = "var(--desktop-muted)"
  const want = getComputedStyle(probe)
  const scale = {
    size: want.width,
    radius: want.borderTopLeftRadius,
    muted: want.color,
  }
  probe.remove()
  const style = getComputedStyle(button)
  const box = button.getBoundingClientRect()
  const product = button.querySelector(".desktop-identity-words > span")
  const failures = []
  if (Math.abs(box.height - Number.parseFloat(scale.size)) > 0.5)
    failures.push(`its pill is ${box.height}px tall, not ${scale.size}`)
  if (style.borderTopLeftRadius !== scale.radius)
    failures.push(`its corner is ${style.borderTopLeftRadius}, not ${scale.radius}`)
  if (!/^nessa \S/.test(button.textContent.trim()))
    failures.push(`it reads "${button.textContent.trim()}", not "nessa" and a word`)
  if (!product || getComputedStyle(product).color !== scale.muted)
    failures.push(`its product word is not --desktop-muted at rest`)
  if (!button.getAttribute("aria-label")) failures.push("it has no accessible name")
  const row = button.closest(".desktop-identity")?.getBoundingClientRect() ?? null
  return {
    failures,
    text: button.textContent.trim(),
    box: { x: box.x, y: box.y, height: box.height },
    row: row && { x: row.x, y: row.y, height: row.height },
  }
}

/** The identity's fill under the pointer, against `--desktop-hover`. */
async function identityHover(page, scale) {
  const button = page.locator(".desktop-identity-button:visible").first()
  await button.hover()
  await page.waitForTimeout(250)
  const fill = await button.evaluate(
    (element) => getComputedStyle(element).backgroundColor,
  )
  await page.mouse.move(400, 400)
  return fill === scale.hover
    ? []
    : [`under the pointer it is ${fill}, not ${scale.hover}`]
}

/**
 * Every empty state on the page against the window's skin of the kit's
 * `EmptyState` (`styles.css` › Empty states): a quiet one's line faint
 * footnote type, a titled one's title at reading size. In the page.
 */
function measureEmptyStates() {
  const probe = document.createElement("div")
  document.querySelector("[data-surface]").append(probe)
  const read = (property, value) => {
    probe.style.setProperty(property, value)
    return getComputedStyle(probe).getPropertyValue(property)
  }
  const want = {
    faint: read("color", "var(--desktop-faint)"),
    footnote: read("font-size", "var(--desktop-text-footnote)"),
    foreground: read("color", "var(--foreground)"),
    read: read("font-size", "var(--desktop-text-read)"),
  }
  probe.remove()
  const failures = []
  const seen = []
  const shown = (element) => element.getClientRects().length > 0
  for (const empty of document.querySelectorAll('[data-slot="empty-state"]')) {
    if (!shown(empty)) continue
    const title = empty.querySelector('[data-slot="empty-state-title"]')
    const style = getComputedStyle(title)
    const words = title.textContent.trim()
    const quiet = empty.dataset.variant === "compact"
    seen.push({ words, variant: empty.dataset.variant, color: style.color })
    const [color, size, weight] = quiet
      ? [want.faint, want.footnote, "400"]
      : [want.foreground, want.read, "600"]
    if (style.color !== color)
      failures.push(`"${words}": ink ${style.color}, not ${color}`)
    if (style.fontSize !== size)
      failures.push(`"${words}": ${style.fontSize}, not ${size}`)
    if (style.fontWeight !== weight)
      failures.push(`"${words}": weight ${style.fontWeight}, not ${weight}`)
  }
  // The classes the window drew its own empty states with, before the kit's.
  for (const old of document.querySelectorAll(
    ".workspace-list-empty, .workspace-empty, .desktop-empty-note, .agents-overview-resting, .agents-clear",
  ))
    if (shown(old) && old.dataset.slot !== "empty-state")
      failures.push(`"${old.textContent.trim()}" is not the kit's EmptyState`)
  return { failures, seen }
}

/**
 * The window's counts and lit points: a count is the kit's `Badge` skinned as
 * a row's caption (`source-list.css`), a point is `StatusGlyph`'s — 6px, in
 * the needs or the running light, wherever it stands. In the page.
 */
function measureBadges() {
  const probe = document.createElement("div")
  document.querySelector("[data-surface]").append(probe)
  const read = (value) => {
    probe.style.background = value
    return getComputedStyle(probe).backgroundColor
  }
  const light = {
    "needs-you": read("var(--desktop-needs)"),
    unread: read("var(--desktop-run)"),
  }
  probe.remove()
  const failures = []
  const shown = (element) => element.getClientRects().length > 0
  const counts = [...document.querySelectorAll(".workspace-badge")].filter(shown)
  for (const count of counts) {
    const box = count.getBoundingClientRect()
    const style = getComputedStyle(count)
    const words = count.textContent.trim()
    if (Math.abs(box.height - 16) > 0.5)
      failures.push(`count ${words}: ${box.height}px tall, not 16`)
    if (box.width < 17.5) failures.push(`count ${words}: ${box.width}px wide, under 18`)
    if (style.borderTopWidth !== "0px")
      failures.push(`count ${words}: a ${style.borderTopWidth} border`)
    if (count.dataset.tone === "needs" && style.color !== light["needs-you"])
      failures.push(`count ${words}: ink ${style.color}, not the needs light`)
  }
  const points = [
    ...document.querySelectorAll(
      '.workspace-status:is([data-status="needs-you"], [data-status="unread"])',
    ),
  ].filter(shown)
  for (const point of points) {
    const dot = getComputedStyle(point, "::before")
    const where = point.closest("h2, [data-session-row], [data-row], [role='option']")
    const name = `${point.dataset.status} point in "${(where?.textContent ?? "").trim().slice(0, 30)}"`
    if (dot.width !== "6px" || dot.height !== "6px")
      failures.push(`${name}: ${dot.width} by ${dot.height}, not 6px`)
    if (dot.backgroundColor !== light[point.dataset.status])
      failures.push(`${name}: ${dot.backgroundColor}, not its light`)
  }
  // The classes the window drew its own point and count with, before.
  for (const old of document.querySelectorAll(".workspace-unread"))
    failures.push(`"${old.outerHTML.slice(0, 60)}" is not StatusGlyph`)
  const header = document.querySelector("#agents-needs-you")
  if (header && getComputedStyle(header, "::before").content !== "none")
    failures.push("the overview's Needs you heading draws its own point")
  return {
    failures,
    counts: counts.length,
    points: points.map((point) => point.dataset.status),
  }
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
  identity: async (page, { open }) => {
    const scale = await page.evaluate(windowScale)
    const failures = []
    const inWindow = await page.evaluate(measureIdentity)
    if (!inWindow) return { failures: ["no identity at the sidebar's foot"] }
    failures.push(...inWindow.failures.map((f) => `"${inWindow.text}": ${f}`))
    failures.push(
      ...(await identityHover(page, scale)).map((f) => `"${inWindow.text}": ${f}`),
    )
    await page.keyboard.press(keys.settings)
    await need(page, css.settings, "Settings")
    await page.waitForTimeout(500)
    const inSettings = await page.evaluate(measureIdentity)
    if (!inSettings) failures.push("no identity at Settings' foot")
    else {
      failures.push(...inSettings.failures.map((f) => `"${inSettings.text}": ${f}`))
      // Where "nessa Studio" opened Settings, the way back stands in the same row.
      if (!inSettings.row || !inWindow.row) failures.push("an identity outside its row")
      else
        for (const side of ["y", "height"])
          if (Math.abs(inSettings.row[side] - inWindow.row[side]) > 0.5)
            failures.push(
              `Settings' row is at ${side} ${inSettings.row[side]}, the window's at ${inWindow.row[side]}`,
            )
    }
    const classic = await open("classic")
    let inClassic
    try {
      inClassic = await classic.evaluate(measureIdentity)
      if (!inClassic) failures.push("no identity in the classic shell")
      else {
        failures.push(
          ...inClassic.failures.map((f) => `classic "${inClassic.text}": ${f}`),
        )
        failures.push(
          ...(await identityHover(classic, scale)).map((f) => `classic: ${f}`),
        )
      }
    } finally {
      await classic
        .context()
        .close()
        .catch(() => {})
    }
    return { failures, measured: { inWindow, inSettings, inClassic } }
  },
  badges: async (page) => {
    const failures = []
    const onLoad = await page.evaluate(measureBadges)
    failures.push(...onLoad.failures)
    if (onLoad.counts === 0) failures.push("no count in the sidebar")
    await page.keyboard.press(keys.overview)
    await need(page, "#agents-needs-you", "the overview's Needs you heading")
    const inOverview = await page.evaluate(measureBadges)
    failures.push(...inOverview.failures.map((f) => `in the overview: ${f}`))
    return { failures, measured: { onLoad, inOverview } }
  },
  empty: async (page, { layout, open }) => {
    const failures = []
    let inList = { seen: [] }
    // The session list is a column of its own only in the three-column layout.
    if (layout === "columns") {
      await page
        .locator(css.listSearch)
        .locator("input")
        .fill("no session is called this")
      await need(page, `${css.listScroll} > *`, "the list's empty state")
      inList = await page.evaluate(measureEmptyStates)
      failures.push(...inList.failures)
      if (inList.seen.length === 0) failures.push("no kit EmptyState in the session list")
    }
    const classic = await open("classic")
    let inClassic
    try {
      inClassic = await classic.evaluate(measureEmptyStates)
      failures.push(...inClassic.failures.map((f) => `classic ${f}`))
      if (inClassic.seen.length < 2)
        failures.push("the classic shell's two notes are not shown")
    } finally {
      await classic
        .context()
        .close()
        .catch(() => {})
    }
    return { failures, measured: { inList: inList.seen, inClassic: inClassic?.seen } }
  },
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
Checks, per engine and layout (--only badges,empty,identity,keys,rows):
  badges     every count is the kit's Badge as a row's caption (16px tall, at
             least 18 wide, no border, the needs light for what waits); every
             lit point — unread, needs you, the overview's heading included —
             is StatusGlyph's 6px point in its light. On load and in the overview.
  empty      every empty state — the session list's with no match, the classic
             shell's notes — is the kit's EmptyState in the window's type: a
             quiet one faint footnote, a titled one at reading size, 600.
  identity   "nessa Studio" at the sidebar's foot, "‹ nessa Agent" at Settings'
             and the classic shell's are one control (ui/identity.tsx): a pill
             the corner controls' size and radius, "nessa" and a word in
             --desktop-muted, named, filled with --desktop-hover under the
             pointer; Settings' row stands where the window's does.
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
            const open = async (other) =>
              (await openPage(browser, { url, layout: other })).page
            try {
              await need(page, css.anyReady, "the window")
              return await checks[name](page, { layout, open })
            } finally {
              await close().catch(() => {})
            }
          })
    })
  },
)
