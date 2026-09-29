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
 *
 * With --shots <dir>, screenshots of each width go there for a person to look at.
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"

import { attempt, CannotRun, chosen } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, keys, names } from "./lib/selectors.mjs"
import { frames, hideColumns, openPanes, settled } from "./lib/workspace.mjs"

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

const meta = {
  name: "responsive",
  summary: "approval card, composer controls, column titles and Settings at many widths",
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
