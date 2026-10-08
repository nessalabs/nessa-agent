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
import { css, keys, names } from "./lib/selectors.mjs"

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
function rowsOnPage(sel) {
  const kit = (element) => element.matches(sel.kitRow)
  const listRow = (element) => element.matches(sel.listRow)
  const label = (element) => element.querySelector(sel.rowLabel)
  const rows = [
    ...document.querySelectorAll(
      [
        `${sel.sidebar} [data-row]`,
        `${sel.sidebar} ${sel.kitRow}`,
        sel.sessionListRow,
        `${sel.overviewItem}:not(${sel.overviewRequest})`,
        `${sel.switcherResults} [role='option']`,
      ].join(", "),
    ),
  ].filter((row) => row.getBoundingClientRect().height > 0 && !row.dataset.section)
  return rows.map((row) => {
    // What a kit row lays beside its control (a count, a glyph) is said once, in
    // the control's name: the thing beside it stays silent.
    const beside = kit(row)
      ? row
          .closest(sel.kitRowFrame)
          ?.querySelector(`${sel.countBadge}, .workspace-status`)
      : null
    const name = row.getAttribute("aria-label") ?? ""
    const words = label(row)?.textContent.trim() ?? ""
    const shown = beside?.textContent.trim() || beside?.dataset.status
    const besideFailure = !beside
      ? null
      : beside.getAttribute("aria-hidden") !== "true"
        ? `"${shown}" beside it is said again`
        : beside.matches(sel.countBadge)
          ? name.includes(beside.textContent.trim())
            ? null
            : `its name does not say "${shown}"`
          : name.startsWith(words) && name !== words
            ? null
            : `its name does not say its ${shown} glyph`
    return {
      text: (words || row.textContent.trim()).slice(0, 40),
      component: kit(row) ? "kit" : listRow(row) ? "list-row" : "other",
      background: getComputedStyle(row).backgroundColor,
      weight: label(row) ? getComputedStyle(label(row)).fontWeight : null,
      unread: row.hasAttribute("data-unread"),
      current:
        row.getAttribute("data-active") === "true" || row.hasAttribute("data-selected"),
      beside: besideFailure,
      besides: beside ? (beside.matches(sel.countBadge) ? "count" : "glyph") : null,
    }
  })
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
    if (row.beside) failures.push(`${where}: "${row.text}": ${row.beside}`)
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
function measureIdentity([selector, sel]) {
  const button = [...document.querySelectorAll(selector)].find(
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
  const product = button.querySelector(sel.identityProduct)
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
  const row = button.closest(sel.identityRow)?.getBoundingClientRect() ?? null
  return {
    failures,
    text: button.textContent.trim(),
    box: { x: box.x, y: box.y, height: box.height },
    row: row && { x: row.x, y: row.y, height: row.height },
  }
}

/** The identity's fill under the pointer, against `--desktop-hover`. */
async function identityHover(page, scale, selector) {
  const button = page.locator(`${selector}:visible`).first()
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
function measureEmptyStates(sel) {
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
  for (const empty of document.querySelectorAll(sel.emptyState)) {
    if (!shown(empty)) continue
    const title = empty.querySelector(sel.emptyTitle)
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
function measureBadges(sel) {
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
  const counts = [...document.querySelectorAll(sel.countBadge)].filter(shown)
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
  const points = [...document.querySelectorAll(sel.litPoint)].filter(shown)
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
  const header = document.querySelector(sel.needsYouHeading)
  if (header && getComputedStyle(header, "::before").content !== "none")
    failures.push("the overview's Needs you heading draws its own point")
  return {
    failures,
    headerPoint: Boolean(header?.querySelector(sel.litPoint)),
    counts: counts.length,
    points: points.map((point) => point.dataset.status),
  }
}

/** The cards set into a surface, which share one edge (`--desktop-card-rim`). */
const cards = [css.codeBlock, css.approvalCommand, css.widgetCard, css.allClear]

/** Every card on the page against `--desktop-card-rim`, by kind. In the page. */
function measureCardRims(selectors) {
  const probe = document.createElement("div")
  document.querySelector("[data-surface]").append(probe)
  probe.style.boxShadow = "var(--desktop-card-rim)"
  const want = getComputedStyle(probe).boxShadow
  probe.remove()
  const failures = []
  const seen = {}
  for (const selector of selectors)
    for (const card of document.querySelectorAll(selector)) {
      if (card.getClientRects().length === 0) continue
      seen[selector] = (seen[selector] ?? 0) + 1
      const rim = getComputedStyle(card).boxShadow
      if (rim !== want) failures.push(`${selector}: its edge is ${rim}, not ${want}`)
    }
  return { failures, seen, want }
}

const rowKinds = {
  columns: {
    "sidebar row": `${css.sidebar} ${css.kitRow}`,
    "pinned session": `${css.sidebar} [data-row='session']`,
    "session list row": css.sessionListRow,
  },
  sidebar: {
    "channel row": `${css.sidebar} [data-row='channel']`,
    "branch session": `${css.sidebar} [data-row='session']`,
  },
}

const checks = {
  identity: async (page, { open }) => {
    const scale = await page.evaluate(windowScale)
    const failures = []
    const inWindow = await page.evaluate(measureIdentity, [css.studio, css])
    if (!inWindow) return { failures: ["no identity at the sidebar's foot"] }
    failures.push(...inWindow.failures.map((f) => `"${inWindow.text}": ${f}`))
    failures.push(
      ...(await identityHover(page, scale, css.studio)).map(
        (f) => `"${inWindow.text}": ${f}`,
      ),
    )
    await page.keyboard.press(keys.settings)
    await need(page, css.settings, "Settings")
    await page.waitForTimeout(500)
    // Measured inside Settings: the window under it keeps its own, inert but drawn.
    const inSettings = await page.evaluate(measureIdentity, [
      `${css.settings} ${css.identityButton}`,
      css,
    ])
    if (!inSettings) failures.push("no identity at Settings' foot")
    else {
      failures.push(...inSettings.failures.map((f) => `"${inSettings.text}": ${f}`))
      if (inSettings.text !== "nessa Agent")
        failures.push(`Settings' foot reads "${inSettings.text}", not "nessa Agent"`)
      failures.push(
        ...(
          await identityHover(page, scale, `${css.settings} ${css.identityButton}`)
        ).map((f) => `"${inSettings.text}": ${f}`),
      )
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
      inClassic = await classic.evaluate(measureIdentity, [css.identityButton, css])
      if (!inClassic) failures.push("no identity in the classic shell")
      else {
        failures.push(
          ...inClassic.failures.map((f) => `classic "${inClassic.text}": ${f}`),
        )
        failures.push(
          ...(await identityHover(classic, scale, css.identityButton)).map(
            (f) => `classic: ${f}`,
          ),
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
  badges: async (page, { layout }) => {
    const failures = []
    const onLoad = await page.evaluate(measureBadges, css)
    failures.push(...onLoad.failures)
    if (onLoad.counts === 0) failures.push("no count in the sidebar")
    // The session list, a column only in this layout, holds the sample's unread session.
    if (layout === "columns" && !onLoad.points.includes("unread"))
      failures.push("no unread point in the session list")
    await page.keyboard.press(keys.overview)
    await need(page, css.needsYouHeading, "the overview's Needs you heading")
    const inOverview = await page.evaluate(measureBadges, css)
    failures.push(...inOverview.failures.map((f) => `in the overview: ${f}`))
    if (!inOverview.headerPoint)
      failures.push("the overview's Needs you heading has no point")
    return { failures, measured: { onLoad, inOverview } }
  },
  segmented: async (page) => {
    const failures = []
    const scale = await page.evaluate(windowScale)
    await page.keyboard.press(keys.overview)
    await need(page, css.overviewCount, "the overview's counts")
    await page.waitForTimeout(400)
    const read = () =>
      page.evaluate((sel) => {
        const options = [...document.querySelectorAll(sel.overviewCount)]
        return {
          group: options[0]?.closest(sel.segmentedControl)?.getAttribute("role"),
          options: options.map((option) => ({
            kit: option.dataset.slot === "segmented-control-option",
            pressed: option.getAttribute("aria-pressed") === "true",
            fill: getComputedStyle(option).backgroundColor,
            box: [...Object.values(option.getBoundingClientRect().toJSON())].map(
              (n) => Math.round(n * 10) / 10,
            ),
          })),
        }
      }, css)
    const before = await read()
    if (before.group !== "group")
      failures.push("the counts are not the kit's segmented control")
    if (before.options.some((option) => !option.kit))
      failures.push("a count is not the kit's segmented option")
    if (before.options.some((option) => option.pressed))
      failures.push("a count is pressed while every group shows")
    const second = page.locator(css.overviewCount).nth(1)
    await second.hover()
    await page.waitForTimeout(250)
    const hover = await second.evaluate(
      (element) => getComputedStyle(element).backgroundColor,
    )
    if (hover !== scale.hover)
      failures.push(`a count under the pointer is ${hover}, not ${scale.hover}`)
    await second.click()
    await page.mouse.move(1, 1)
    await page.waitForTimeout(400)
    const pressed = await read()
    const chosen = pressed.options[1]
    if (!chosen?.pressed) failures.push("the chosen count is not pressed")
    else if (chosen.fill !== scale.selected)
      failures.push(`the pressed count is ${chosen.fill}, not ${scale.selected}`)
    // Every count has the same box pressed or not, so choosing one moves nothing.
    if (
      JSON.stringify(pressed.options.map((o) => o.box)) !==
      JSON.stringify(before.options.map((o) => o.box))
    )
      failures.push("choosing a count moved the counts")
    await page.locator(css.overviewCount).nth(1).click()
    await page.waitForTimeout(400)
    const released = await read()
    if (released.options.some((option) => option.pressed))
      failures.push("choosing the pressed count again did not let it go")
    return {
      failures,
      measured: { counts: before.options.length, hover, pressed: chosen?.fill },
    }
  },
  rims: async (page) => {
    const failures = []
    const seen = {}
    const measure = async (where) => {
      const result = await page.evaluate(measureCardRims, cards)
      failures.push(...result.failures.map((f) => `${where}: ${f}`))
      for (const [kind, count] of Object.entries(result.seen))
        seen[kind] = (seen[kind] ?? 0) + count
    }
    await measure("on load")
    // The sample's widget cards, in a conversation.
    await page.keyboard.press(keys.switcher)
    await page.waitForSelector(css.switcherField, { state: "visible" })
    await page.keyboard.type(names.widgetSession)
    await page.keyboard.press(keys.enter)
    await need(page, css.widgetCard, "a widget's card")
    await measure("in the widget sample")
    // An agent's command, in the overview's peek.
    await page.keyboard.press(keys.overview)
    await need(page, css.overviewRequest, "a request in the overview")
    await page.locator(css.overviewRequest).first().click()
    await need(page, css.peekCommand, "the command in the peek")
    await measure("in the overview")
    // "Nothing needs you" shows only with nothing listed, which the sample never is.
    for (const kind of [css.codeBlock, css.approvalCommand, css.widgetCard])
      if (!seen[kind]) failures.push(`no ${kind} was shown to measure`)
    return { failures, measured: seen }
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
      inList = await page.evaluate(measureEmptyStates, css)
      failures.push(...inList.failures)
      if (inList.seen.length === 0) failures.push("no kit EmptyState in the session list")
    }
    const classic = await open("classic")
    let inClassic
    try {
      inClassic = await classic.evaluate(measureEmptyStates, css)
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
    const onLoad = await page.evaluate(rowsOnPage, css)
    failures.push(...rowFailures(onLoad, scale, "on load"))
    // The sample's unread session shows in the session list and in the tree.
    if (!onLoad.some((row) => row.unread)) failures.push("no unread row on load to weigh")
    // A count and a glyph beside a kit row, each said once (the sample has both).
    for (const kind of ["count", "glyph"])
      if (!onLoad.some((row) => row.besides === kind))
        failures.push(`no kit row with a ${kind} beside it on load`)
    // Pointed at, a count or a running glyph at a row's end leaves the row its fill.
    for (const kind of ["count", "glyph"]) {
      const beside = page
        .locator(
          `${css.sidebar} [data-trailing="${kind}"] + [data-slot="sidebar-menu-item-trailing"] :is(${css.countBadge}, .workspace-status)`,
        )
        .first()
      if (!(await beside.count())) {
        // The tree lays its folded summary in the kit's own badge slot, with no `data-trailing`.
        measured.hover[`kit row under its ${kind}`] = "none on this layout"
        if (kind === "count" || layout === "columns")
          failures.push(`no kit row with a ${kind} at its end to point at`)
        continue
      }
      await beside.hover({ force: true })
      await page.waitForTimeout(250)
      const fill = await beside.evaluate(
        (element, sel) =>
          getComputedStyle(element.closest(sel.kitRowFrame).querySelector(sel.kitRow))
            .backgroundColor,
        css,
      )
      measured.hover[`kit row under its ${kind}`] = fill
      if (fill !== scale.hover && fill !== scale.selected)
        failures.push(
          `a kit row pointed at by its ${kind} is ${fill}, not ${scale.hover}`,
        )
      await page.mouse.move(1, 1)
    }
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
    const inOverview = await page.evaluate(rowsOnPage, css)
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
    const inSwitcher = await page.evaluate(rowsOnPage, css)
    failures.push(...rowFailures(inSwitcher, scale, "in the switcher"))
    if (!inSwitcher.some((row) => row.current))
      failures.push("no chosen row in the switcher")
    // The switcher's choice follows the pointer, so a result draws no hover of its
    // own: pointed at, then left behind by the keys, a row is back at rest.
    const results = page.locator(`${css.switcherResults} [role='option']`)
    await results.nth(2).hover()
    await page.keyboard.press(keys.down)
    await page.keyboard.press(keys.down)
    await page.waitForTimeout(250)
    const left = await results
      .nth(2)
      .evaluate((row) => [
        row.getAttribute("aria-selected"),
        getComputedStyle(row).backgroundColor,
      ])
    measured.hover["switcher row left by the keys"] = left[1]
    if (left[0] === "true")
      failures.push("the keys did not move the switcher's choice off the row pointed at")
    else if (left[1] !== "rgba(0, 0, 0, 0)")
      failures.push(
        `a switcher row the keys left is ${left[1]} under the pointer, not at rest`,
      )
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
Checks, per engine and layout (--only badges,empty,identity,keys,rims,rows,segmented):
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
  rims       every card set into a surface — a code block, an agent's command,
             a widget's card, "Nothing needs you" — has --desktop-card-rim for
             its edge. On load, in the widget sample, in the overview's peek.
  rows       every row of the window's lists — the sidebar's, the session list's,
             the overview's sessions and the switcher's — is the kit's
             SidebarMenuItem or ListRow; under the pointer each kind fills with
             --desktop-hover, a chosen row with --desktop-selected, and an unread
             title weighs --desktop-unread-weight. On load, in the overview and
             with the switcher open.
  segmented  the overview's counts are the kit's SegmentedControl (bare): none
             pressed while every group shows, --desktop-hover under the pointer,
             --desktop-selected pressed, nothing moves as one is chosen, and the
             pressed one chosen again lets go.

Settings' own controls are left out (#632 › Settings is redesigned on its own
branch); its identity at the foot is measured.`,
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
