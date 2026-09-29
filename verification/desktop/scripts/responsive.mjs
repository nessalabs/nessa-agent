#!/usr/bin/env node
/**
 * Layout at the widths people use:
 *
 *   approval-card   the card, arranged by its own width (ADR 238, ui/ — the
 *                   approval card), at 280/340/420/600/900 px, with a short and a
 *                   very long command: no word of the command broken, no button
 *                   label wrapped, nothing overflowing the card
 *   composer-chips  four panes while the window narrows: no two composer
 *                   controls overlap
 *   column-title    each column's title inline in the titlebar row where it fits,
 *                   below it where it does not ("The titlebar's safe area"): an
 *                   inline title never overlaps a titlebar control
 *   settings-fold   Settings' sidebar folds for room below a page of 420px
 *   list-gutter     the session list's search field and a row sit as far from what
 *                   is on their left — the sidebar's card, or with the sidebar
 *                   away the window's edge — as from the panes on their right
 *   overview-header the Agents overview's header holds its place while its list
 *                   scrolls to the end, the list begins below it, and the list's
 *                   top edge fades (its mask) rather than cutting a row
 *   thinking-control the composer's thinking control (ADR 238 › The thinking
 *                   control), from the keyboard alone: it opens onto the level
 *                   chosen, the arrows, Home and End change the level and the
 *                   keyboard follows, and in every frame of every change — Fast
 *                   too — the composer, its controls and the popover hold their
 *                   place; a level's change animates transform and opacity only;
 *                   Escape, and Tab past its end, close it onto its chip; with
 *                   the system's reduced motion, nothing animates and the words
 *                   that were shown are not drawn
 *
 * With --shots <dir>, screenshots of each width go there for a person to look at.
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"

import { attempt, CannotRun, chosen } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, keys, names } from "./lib/selectors.mjs"
import { frames, hideColumns, openPanes, settled, until } from "./lib/workspace.mjs"

const longTail =
  " --config=/Users/nessa/Library/Application-Support/nessa/releases/very/deep/path/release-signing-configuration.json"

const shot = async (options, page, name, locator) => {
  if (!options.shots) return
  mkdirSync(options.shots, { recursive: true })
  const path = join(options.shots, `${name}.png`)
  if (locator) await locator.screenshot({ path })
  else await page.screenshot({ path })
}

const checks = {
  "approval-card": async ({ page, engine, options }) => {
    const row = page.locator(css.sessionRow, { hasText: names.approvalSession }).first()
    if (!(await row.count()))
      throw new CannotRun(`no session row "${names.approvalSession}" in the list`)
    await row.click()
    await need(page, css.approvalCard, "the approval card")
    await settled(page)
    const failures = []
    const seen = []
    for (const long of [false, true])
      for (const width of [280, 340, 420, 600, 900]) {
        await page.evaluate(
          ([sel, width, long, tail]) => {
            let style = document.getElementById("__verify_card")
            if (!style) {
              style = document.createElement("style")
              style.id = "__verify_card"
              document.head.append(style)
            }
            style.textContent = `${sel.approvalCard} { width: ${width}px; box-sizing: border-box; }`
            const words = document.querySelectorAll(sel.approvalWord)
            if (long && !document.getElementById("__verify_long") && words.length) {
              const extra = document.createElement("span")
              extra.id = "__verify_long"
              extra.className = words[0].className
              extra.textContent = tail
              words[words.length - 1].after(extra)
            }
          },
          [css, width, long, longTail],
        )
        // The container queries apply in the next frames' style and layout.
        await frames(page, 2)
        const r = await page.evaluate((sel) => {
          const card = document.querySelector(sel.approvalCard)
          const buttons = [...card.querySelectorAll(sel.approvalActions)].filter((b) =>
            b.checkVisibility(),
          )
          const rows = new Map()
          for (const b of buttons) {
            const top = Math.round(b.getBoundingClientRect().top)
            rows.set(top, [...(rows.get(top) ?? []), b.textContent.trim()])
          }
          const lineHeight = (b) => parseFloat(getComputedStyle(b).lineHeight) || 20
          return {
            card: Math.round(card.getBoundingClientRect().width),
            rows: [...rows.values()].map((r) => r.join(" + ")),
            broken: [...card.querySelectorAll(sel.approvalWord)]
              .filter((w) => w.id !== "__verify_long" && w.getClientRects().length > 1)
              .map((w) => w.textContent.trim()),
            wrapped: buttons
              .filter((b) => b.getBoundingClientRect().height > lineHeight(b) * 1.9)
              .map((b) => b.textContent.trim()),
            overflow: card.scrollWidth > card.clientWidth + 1,
          }
        }, css)
        const tag = `${width}px${long ? " long" : ""}`
        seen.push({ width, long, ...r })
        if (r.broken.length)
          failures.push(
            `${tag}: command word(s) broken across lines: ${r.broken.join(", ")}`,
          )
        if (r.wrapped.length)
          failures.push(`${tag}: button label(s) wrapped: ${r.wrapped.join(", ")}`)
        if (r.overflow) failures.push(`${tag}: the card overflows`)
        await shot(
          options,
          page,
          `card-${engine}-${long ? "long-" : ""}${width}`,
          page.locator(css.approvalCard).first(),
        )
      }
    return { widths: seen, failures }
  },

  "composer-chips": async ({ page, engine, layout, options }) => {
    await hideColumns(page, layout)
    await openPanes(page, 4)
    const failures = []
    const seen = []
    for (const width of [1440, 1200, 1000, 900, 800, 720, 660, 600]) {
      await page.setViewportSize({ width, height: 900 })
      // The resize reaches the page a frame or two later; then its motion runs out.
      await frames(page, 2)
      await settled(page)
      const overlaps = await page.evaluate((sel) => {
        const out = []
        for (const p of document.querySelectorAll(sel.pane)) {
          const textarea = p.querySelector(sel.field)
          let bar = textarea
          for (let i = 0; i < 6 && bar; i++) {
            bar = bar.parentElement
            if (bar && bar.querySelectorAll(sel.button).length >= 3) break
          }
          if (!bar) continue
          const buttons = [...bar.querySelectorAll(sel.button)].filter((b) =>
            b.checkVisibility({ checkOpacity: true }),
          )
          const boxes = buttons.map((b) => [b, b.getBoundingClientRect()])
          for (let i = 0; i < boxes.length; i++)
            for (let j = i + 1; j < boxes.length; j++) {
              const [a, ra] = boxes[i]
              const [b, rb] = boxes[j]
              if (a.contains(b) || b.contains(a)) continue
              const w = Math.min(ra.right, rb.right) - Math.max(ra.left, rb.left)
              const h = Math.min(ra.bottom, rb.bottom) - Math.max(ra.top, rb.top)
              if (w > 1 && h > 1) {
                const name = (e) =>
                  (e.getAttribute("aria-label") ?? e.textContent).trim().slice(0, 16)
                out.push(
                  `pane ${p.dataset.paneKey} (${Math.round(p.getBoundingClientRect().width)}px): "${name(a)}" overlaps "${name(b)}" by ${Math.round(w)}px`,
                )
              }
            }
        }
        return out
      }, css)
      seen.push({ width, overlaps: overlaps.length })
      for (const o of overlaps) failures.push(`${width}px window: ${o}`)
      await shot(options, page, `composer-${engine}-${layout}-${width}`)
    }
    return { widths: seen, failures }
  },

  "column-title": async ({ page, engine, layout, options }) => {
    const failures = []
    const seen = []
    for (const width of [1440, 1200, 1000, 900, 800, 700]) {
      await page.setViewportSize({ width, height: 900 })
      // The resize reaches the page a frame or two later; then its motion runs out.
      await frames(page, 2)
      await settled(page)
      const r = await page.evaluate((sel) => {
        const titles = [...document.querySelectorAll(sel.columnTitle)].filter((t) =>
          t.checkVisibility(),
        )
        const buttons = [...document.querySelectorAll(sel.titlebarButtons)].filter((b) =>
          b.checkVisibility({ checkOpacity: true }),
        )
        return titles.map((t) => {
          const heading = t.querySelector(sel.heading) ?? t
          const range = document.createRange()
          range.selectNodeContents(heading)
          const tr = range.getBoundingClientRect()
          const hits = buttons
            .map((b) => [b, b.getBoundingClientRect()])
            .filter(
              ([, r]) =>
                Math.min(r.right, tr.right) - Math.max(r.left, tr.left) > 1 &&
                Math.min(r.bottom, tr.bottom) - Math.max(r.top, tr.top) > 1,
            )
            .map(([b]) =>
              (b.getAttribute("aria-label") ?? b.textContent).trim().slice(0, 20),
            )
          return {
            title: heading.textContent.trim().slice(0, 24),
            placement: t.dataset.placement,
            hits,
          }
        })
      }, css)
      seen.push({ width, titles: r.map((t) => `${t.title}:${t.placement}`) })
      for (const t of r)
        if (t.hits.length)
          failures.push(
            `${width}px: "${t.title}" (${t.placement}) overlaps ${t.hits.join(", ")}`,
          )
      await shot(options, page, `titles-${engine}-${layout}-${width}`)
    }
    if (!seen.some((s) => s.titles.length))
      throw new CannotRun(`no column titles found (${css.columnTitle})`)
    return { widths: seen, failures }
  },

  "settings-fold": async ({ page, engine, options }) => {
    await page.keyboard.press(keys.settings)
    await need(page, css.settings, "Settings")
    await settled(page)
    const failures = []
    const seen = []
    for (const width of [1200, 1000, 800, 700, 640, 600, 560]) {
      await page.setViewportSize({ width, height: 800 })
      // The resize reaches the page a frame or two later; then its motion runs out.
      await frames(page, 2)
      await settled(page)
      const r = await page.evaluate((sel) => {
        const s = document.querySelector(sel.settings)
        const sidebar =
          parseFloat(getComputedStyle(s).getPropertyValue("--settings-sidebar-w")) || 0
        return {
          sidebar: s.dataset.sidebar,
          sidebarWidth: sidebar,
          page: Math.round(s.getBoundingClientRect().width - sidebar),
        }
      }, css)
      seen.push({ width, ...r })
      if (r.sidebar === "open" && r.page < 420)
        failures.push(`${width}px: sidebar drawn with a ${r.page}px page (< 420)`)
      await shot(options, page, `settings-${engine}-${width}`)
    }
    return { widths: seen, failures }
  },
}

checks["overview-header"] = async ({ page, engine, options }) => {
  // Short enough that the list must scroll.
  await page.setViewportSize({ width: 1440, height: 560 })
  await page.keyboard.press(keys.overview)
  await need(page, css.overview, "the Agents overview")
  await frames(page, 2)
  await settled(page)
  const read = () =>
    page.evaluate((sel) => {
      const header = document.querySelector(sel.overviewHeader)
      const scroller = document.querySelector(sel.overviewScroll)
      if (!header || !scroller) return null
      const style = getComputedStyle(scroller)
      return {
        headerTop: header.getBoundingClientRect().top,
        headerBottom: header.getBoundingClientRect().bottom,
        scrollerTop: scroller.getBoundingClientRect().top,
        scrollTop: scroller.scrollTop,
        room: scroller.scrollHeight - scroller.clientHeight,
        mask: style.maskImage || style.webkitMaskImage || "",
      }
    }, css)
  const before = await read()
  if (!before) throw new CannotRun(`no ${css.overviewHeader} or ${css.overviewScroll}`)
  if (before.room < 40)
    throw new CannotRun(
      `the list scrolls only ${before.room}px at 1440 × 560: nothing to hold`,
    )
  await page.evaluate((sel) => {
    const scroller = document.querySelector(sel.overviewScroll)
    scroller.scrollTop = scroller.scrollHeight
  }, css)
  await frames(page, 2)
  const after = await read()
  await shot(options, page, `overview-header-${engine}`)
  const failures = []
  if (after.scrollTop < before.room - 1)
    failures.push(`the list scrolled to ${after.scrollTop} of ${before.room}px`)
  if (Math.abs(after.headerTop - before.headerTop) > 0.5)
    failures.push(
      `the header moved ${after.headerTop - before.headerTop}px as the list scrolled`,
    )
  if (after.scrollerTop < after.headerBottom - 0.5)
    failures.push(
      `the list begins at ${after.scrollerTop}px, under the header ending at ${after.headerBottom}px`,
    )
  // The fade: the mask begins transparent at the list's top edge.
  if (!/^linear-gradient\((?:rgba\(0, 0, 0, 0\)|transparent)/.test(after.mask))
    failures.push(`the list's top edge has no fade (mask: "${after.mask}")`)
  return { before, after, failures }
}

checks["list-gutter"] = async ({ page, engine, layout, options }) => {
  if (layout !== "columns")
    return { skipped: "the session list is a column only in the columns layout" }
  await need(page, css.listSearch, "the session list's search field")
  const measure = () =>
    page.evaluate((sel) => {
      const box = (element) => element?.getBoundingClientRect() ?? null
      const sidebar = document.querySelector(sel.workspace)?.dataset.sidebar
      const left =
        sidebar === "open" ? box(document.querySelector(sel.sidebar))?.right : 0
      const panes = [...document.querySelectorAll(sel.pane)].map((pane) => box(pane).left)
      const search = box(document.querySelector(sel.listSearch))
      const row = box(document.querySelector(`${sel.sessionList} ${sel.sessionRow}`))
      return {
        sidebar,
        left,
        right: Math.min(...panes),
        search: search && [search.left, search.right],
        row: row && [row.left, row.right],
      }
    }, css)
  const failures = []
  const seen = []
  for (const state of ["open", "closed"]) {
    if (state === "closed") {
      await page.keyboard.press(keys.toggleSidebar)
      await frames(page, 2)
      await settled(page)
    }
    const m = await measure()
    if (m.sidebar !== state)
      throw new CannotRun(`the sidebar is ${m.sidebar}, not ${state}`)
    if (!m.search || !m.row)
      throw new CannotRun("no search field or session row to measure")
    await shot(options, page, `list-gutter-${engine}-${state}`)
    for (const [name, [start, end]] of [
      ["search field", m.search],
      ["row", m.row],
    ]) {
      const before = start - m.left
      const after = m.right - end
      seen.push({ state, name, before: Math.round(before), after: Math.round(after) })
      if (Math.abs(before - after) > 1)
        failures.push(
          `sidebar ${state}: the ${name} is ${Math.round(before)}px from what is on its left, ${Math.round(after)}px from the panes`,
        )
    }
  }
  return { gutters: seen, failures }
}

/**
 * Samples, every frame until `stop`, where a composer, its controls and the
 * thinking popover are, and what animates in them: how many animations were
 * running and which properties they moved.
 */
const sampleThinking = (page, chip) =>
  page.evaluate(
    ([sel, chipSelector]) => {
      const chipElement = document.querySelector(chipSelector)
      const composer = chipElement?.closest(sel.composerForm)
      const popover = document.querySelector(sel.thinkingPopover)
      if (!composer || !popover) return false
      const controls = [...composer.querySelectorAll(sel.button)]
      const rect = (e) => {
        const r = e.getBoundingClientRect()
        return [r.left, r.top, r.width, r.height]
      }
      const sample = { frames: [], running: 0, properties: [], stop: false }
      window.__thinkingSample = sample
      const properties = new Set()
      const ignored = new Set(["offset", "computedOffset", "easing", "composite"])
      const tick = () => {
        sample.frames.push({
          composer: rect(composer),
          controls: controls.map(rect),
          popover: popover.isConnected ? rect(popover) : null,
        })
        for (const a of document.getAnimations()) {
          const target = a.effect?.target
          if (!(target instanceof Element)) continue
          if (!popover.contains(target) && !composer.contains(target)) continue
          if (a.playState !== "running" && a.playState !== "pending") continue
          sample.running += 1
          if (a.transitionProperty) properties.add(a.transitionProperty)
          else
            for (const frame of a.effect.getKeyframes())
              for (const name of Object.keys(frame))
                if (!ignored.has(name)) properties.add(name)
        }
        sample.properties = [...properties]
        if (!sample.stop) requestAnimationFrame(tick)
      }
      tick()
      return true
    },
    [css, chip],
  )

/** Stops `sampleThinking` and reads what it saw. */
const sampled = (page) =>
  page.evaluate(() => {
    const sample = window.__thinkingSample
    sample.stop = true
    return sample
  })

/** The largest distance anything sampled moved from where it was in the first frame. */
function drift(frames, part) {
  const first = frames[0][part]
  let most = 0
  for (const frame of frames) {
    const now = frame[part]
    if (!first || !now) continue
    const pairs = Array.isArray(first[0])
      ? first.map((r, i) => [r, now[i]])
      : [[first, now]]
    for (const [a, b] of pairs)
      for (let i = 0; i < a.length; i++) most = Math.max(most, Math.abs(a[i] - b[i]))
  }
  return Math.round(most * 100) / 100
}

/** Which stop is checked and which has the keyboard, by position (the levels are the page's). */
const thinkingState = (page, chip) =>
  page.evaluate(
    ([sel, chipSelector]) => {
      const stops = [...document.querySelectorAll(sel.thinkingStop)]
      return {
        open: document.querySelector(sel.thinkingPopover) !== null,
        count: stops.length,
        checked: stops.findIndex((s) => s.getAttribute("aria-checked") === "true"),
        focused: stops.indexOf(document.activeElement),
        chipFocused: document.activeElement === document.querySelector(chipSelector),
        chipLabel: document.querySelector(chipSelector)?.getAttribute("aria-label"),
        checkedLabel: stops.find((s) => s.getAttribute("aria-checked") === "true")
          ?.ariaLabel,
      }
    },
    [css, chip],
  )

/** Opens a new session's home, whose composer offers every level and Fast, and its chip from the keyboard. */
async function openThinking(page) {
  await page.keyboard.press(keys.newSession)
  await settled(page)
  const chip = `${css.focusedPane} ${css.thinkingChip}`
  await need(page, chip, "the focused pane's thinking chip")
  if (await page.locator(chip).isDisabled())
    throw new CannotRun("the new session's model offers no thinking levels")
  await page.locator(chip).focus()
  return chip
}

checks["thinking-control"] = async ({ page, engine, options }) => {
  const failures = []
  const chip = await openThinking(page)
  const moves = []
  const sampling = async (name, act, { motionOnly = true } = {}) => {
    if (!(await sampleThinking(page, chip)))
      throw new CannotRun("no composer or thinking popover to sample")
    await act()
    await settled(page)
    await frames(page, 2)
    const sample = await sampled(page)
    const move = {
      name,
      frames: sample.frames.length,
      running: sample.running,
      properties: sample.properties,
      composer: drift(sample.frames, "composer"),
      controls: drift(sample.frames, "controls"),
      popover: drift(sample.frames, "popover"),
    }
    moves.push(move)
    for (const part of ["composer", "controls", "popover"])
      if (move[part] > 0.5) failures.push(`${name}: the ${part} moved ${move[part]}px`)
    const other = sample.properties.filter((p) => p !== "transform" && p !== "opacity")
    if (motionOnly && other.length)
      failures.push(`${name}: animated ${other.join(", ")} (transform and opacity only)`)
    return move
  }

  await page.keyboard.press(keys.enter)
  if (!(await until(page, (sel) => document.querySelector(sel), css.thinkingPopover)))
    return { failures: ["Return on the chip did not open the thinking popover"] }
  await settled(page)
  const opened = await thinkingState(page, chip)
  if (opened.count < 3)
    throw new CannotRun(`the popover offers ${opened.count} levels; the walk needs 3`)
  if (opened.focused !== opened.checked)
    failures.push(
      `opened with the keyboard on stop ${opened.focused}, not the chosen ${opened.checked}`,
    )
  await shot(options, page, `thinking-${engine}-opened`)

  // Home first, so every step after it has somewhere to go.
  const walk = [
    [keys.home, () => 0],
    [keys.right, (s) => s.checked + 1],
    [keys.right, (s) => s.checked + 1],
    [keys.end, (s) => s.count - 1],
    [keys.left, (s) => s.checked - 1],
    [keys.home, () => 0],
  ]
  let now = opened
  for (const [key, expected] of walk) {
    const want = expected(now)
    await sampling(`${key} to stop ${want}`, () => page.keyboard.press(key))
    now = await thinkingState(page, chip)
    if (now.checked !== want)
      failures.push(`${key}: the level is stop ${now.checked}, not ${want}`)
    if (now.focused !== now.checked)
      failures.push(`${key}: the keyboard is on stop ${now.focused}, not the level's`)
    if (now.chipLabel !== `Thinking level: ${now.checkedLabel}`)
      failures.push(`${key}: the chip says "${now.chipLabel}"`)
    if (key === keys.end) await shot(options, page, `thinking-${engine}-utmost`)
  }
  const changes = moves.filter((m) => m.name !== "fast")
  if (!changes.some((m) => m.running > 0))
    failures.push("no level change animated at all: the sampler saw nothing")

  // Fast, where offered: a setting of its own, and turning it on moves nothing.
  const fast = await page.locator(css.thinkingFast).count()
  if (fast) {
    await sampling(
      "fast",
      async () => {
        await page.locator(css.thinkingFast).focus()
        await page.keyboard.press("Space")
      },
      // Its pill changes colour as well as its bolt moving; only its place is held here.
      { motionOnly: false },
    )
    const pressed = await page.locator(css.thinkingFast).getAttribute("aria-pressed")
    if (pressed !== "true")
      failures.push(`Fast mode's toggle is aria-pressed="${pressed}"`)
    const after = await thinkingState(page, chip)
    if (after.checked !== now.checked)
      failures.push(`Fast changed the level from stop ${now.checked} to ${after.checked}`)
  }

  await page.keyboard.press(keys.escape)
  const closed = await thinkingState(page, chip)
  if (closed.open) failures.push("Escape left the thinking popover open")
  if (!closed.chipFocused)
    failures.push("Escape did not give the keyboard back to the chip")

  // Tab from the level, the popover's last part, leaves it for its chip too.
  await page.keyboard.press(keys.enter)
  await until(page, (sel) => document.querySelector(sel), css.thinkingPopover)
  await page.keyboard.press("Tab")
  const tabbed = await thinkingState(page, chip)
  if (tabbed.open) failures.push("Tab past the popover's end left it open")
  if (!tabbed.chipFocused)
    failures.push("Tab past the popover's end did not land on the chip")

  // With the system's reduced motion, the same walk animates nothing.
  const reduced = await openPage(page.context().browser(), {
    url: page.url(),
    layout: "columns",
    reducedMotion: "reduce",
  })
  const still = []
  try {
    const reducedChip = await openThinking(reduced.page)
    await reduced.page.keyboard.press(keys.enter)
    if (
      !(await until(
        reduced.page,
        (sel) => document.querySelector(sel),
        css.thinkingPopover,
      ))
    )
      throw new CannotRun(
        "Return on the chip did not open the popover with reduced motion",
      )
    // The chip's own wash as it opens is a colour, not the control's motion: let it end.
    await settled(reduced.page)
    for (const key of [keys.end, keys.home, keys.right]) {
      if (!(await sampleThinking(reduced.page, reducedChip)))
        throw new CannotRun("no thinking popover to sample with reduced motion")
      await reduced.page.keyboard.press(key)
      await frames(reduced.page, 6)
      const sample = await sampled(reduced.page)
      still.push({ key, running: sample.running, properties: sample.properties })
      if (sample.running > 0)
        failures.push(
          `reduced motion: ${key} ran ${sample.running} animation frames (${sample.properties.join(", ")})`,
        )
      const leaving = await reduced.page.evaluate(
        (sel) =>
          [...document.querySelectorAll(sel)].map((e) => getComputedStyle(e).opacity),
        css.thinkingLeaving,
      )
      if (leaving.some((opacity) => opacity !== "0"))
        failures.push(`reduced motion: ${key} left the old words drawn (${leaving})`)
    }
    failures.push(...reduced.errors)
  } finally {
    await reduced.close()
  }

  return { opened, moves, reduced: still, failures }
}

const meta = {
  name: "responsive",
  summary:
    "approval card, composer controls and thinking control, column titles and Settings at many widths",
  defaults: { engine: "chromium,webkit", layout: "columns" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/responsive.mjs [options] [--shots <dir>]

  --only <list>   Checks, comma-separated. Available:
                  ${Object.keys(checks).join(", ")}`,
}

await main(meta, async ({ options, rep, url }) => {
  const only = options.only
    ? chosen(options.only, Object.keys(checks), options.list)
    : null
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts)
      for (const [name, check] of Object.entries(checks)) {
        if (only && !only.includes(name)) continue
        await attempt(rep, { name, engine, layout }, async () => {
          const opened = await openPage(browser, {
            url,
            layout,
            width: 1440,
            height: 900,
          })
          try {
            const result = await check({ page: opened.page, engine, layout, options })
            return { ...result, failures: [...(result.failures ?? []), ...opened.errors] }
          } finally {
            await opened.close()
          }
        })
      }
  })
})
