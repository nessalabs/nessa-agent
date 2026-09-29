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
 *                   control): from the keyboard it opens onto the slider, the
 *                   arrows, Home and End change the level, and only Ultra, past
 *                   Max, is marked apart; dragged, the knob follows the pointer,
 *                   the level is the nearest and let go it settles on it; its
 *                   popover stays on its chip as ⌘B and ⌥⌘S move it; on a model
 *                   that ends at Max, the walk ends there, Ultra carried to it
 *                   shows as Max, and Max chosen there stays Max; in
 *                   every frame of every change — the drag and Fast too — the
 *                   composer, its controls and the popover hold their
 *                   place; a level's change animates transform and opacity only;
 *                   Escape, and Tab past its end, close it onto its chip; with
 *                   the system's reduced motion, nothing animates and the words
 *                   that were shown are not drawn
 *   home-shape      a new session's home in a pane smaller than 640px either
 *                   way takes a conversation's shape: its composer docked at
 *                   the foot as the conversation's beside it is, with the
 *                   greeting on and off, the scene gone, "Working late?" in
 *                   the middle at the conversation title's size; a larger
 *                   pane keeps the scene and the card; a home appearing plays
 *                   no settling, and each crossing plays one, by opacity and
 *                   transform alone, and none with less motion; the same
 *                   field, its draft, the model and an open page survive each
 *                   crossing, and the caret's focus and position each resize
 *
 * With --shots <dir>, screenshots of each width go there for a person to look at.
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"

import { attempt, CannotRun, chosen } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, keys, names, preferenceEvents, storage } from "./lib/selectors.mjs"
import {
  frames,
  hideColumns,
  openPanes,
  paneCount,
  paneCountIs,
  settled,
  until,
} from "./lib/workspace.mjs"

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
  "home-shape": async ({ page, engine, layout, options }) => {
    const failures = []
    const seen = []
    const draft = "A draft that crosses the threshold"
    // Where the caret is put before a crossing, and must be after it: not the end.
    const caretAt = draft.length - 5
    const tolerance = 1.5
    // The home's shape, and the conversation's beside it, as drawn.
    const measure = () =>
      page.evaluate((sel) => {
        const box = (e) => {
          if (!e) return null
          const r = e.getBoundingClientRect()
          return { x: r.x, y: r.y, w: r.width, h: r.height }
        }
        const type = (e) => {
          if (!e) return null
          const s = getComputedStyle(e)
          return { size: s.fontSize, weight: s.fontWeight, tracking: s.letterSpacing }
        }
        const home = document.querySelector(sel.paneHome)
        const homePane = home?.closest(sel.pane)
        const other = [...document.querySelectorAll(sel.pane)].find(
          (p) => p !== homePane && p.querySelector(sel.conversationDock),
        )
        const field = home?.querySelector(sel.field)
        const scene = document.querySelector(sel.homeScene)
        return {
          pane: box(homePane),
          body: box(homePane?.querySelector(sel.paneBody)),
          composer: box(home?.querySelector(sel.composerCard)),
          greeting: box(home?.querySelector(".desktop-greeting")),
          greetingType: type(document.querySelector(sel.greeting)),
          scene: scene ? scene.checkVisibility() : false,
          text: field?.value ?? null,
          focused: !!field && document.activeElement === field,
          caret: field?.selectionStart ?? null,
          // The field the crossings began with, marked below: the same one, not a new one.
          same: field?.__homeShapeField === true,
          model: home?.querySelector(sel.modelChip)?.textContent ?? null,
          page: home?.querySelector(sel.homePage) !== null,
          beside: other
            ? {
                pane: box(other),
                composer: box(
                  other.querySelector(`${sel.conversationDock} ${sel.composerCard}`),
                ),
                titleType: type(other.querySelector(sel.conversationTitle)),
              }
            : null,
        }
      }, css)
    const docked = (m, tag) => {
      if (!m.pane || !m.composer) return failures.push(`${tag}: no home in a pane`)
      const foot = m.pane.y + m.pane.h - (m.composer.y + m.composer.h)
      if (m.beside?.composer) {
        const theirs =
          m.beside.pane.y + m.beside.pane.h - (m.beside.composer.y + m.beside.composer.h)
        if (Math.abs(foot - theirs) > tolerance)
          failures.push(
            `${tag}: the home's composer is ${Math.round(foot)}px from its pane's foot, the conversation's ${Math.round(theirs)}px`,
          )
        if (Math.abs(m.composer.h - m.beside.composer.h) > tolerance)
          failures.push(
            `${tag}: the home's composer is ${Math.round(m.composer.h)}px tall, the conversation's ${Math.round(m.beside.composer.h)}px`,
          )
        const inset = (c, p) => c.x - p.x
        if (
          Math.abs(inset(m.composer, m.pane) - inset(m.beside.composer, m.beside.pane)) >
          tolerance
        )
          failures.push(
            `${tag}: the home's composer is inset ${Math.round(inset(m.composer, m.pane))}px, the conversation's ${Math.round(inset(m.beside.composer, m.beside.pane))}px`,
          )
        const a = m.greetingType
        const b = m.beside.titleType
        if (
          a &&
          b &&
          (a.size !== b.size || a.weight !== b.weight || a.tracking !== b.tracking)
        )
          failures.push(
            `${tag}: greeting set ${JSON.stringify(a)}, the conversation's title ${JSON.stringify(b)}`,
          )
      } else if (foot > 24)
        failures.push(
          `${tag}: the home's composer is ${Math.round(foot)}px from its pane's foot`,
        )
      if (m.scene) failures.push(`${tag}: the scene is shown in a small pane`)
      if (m.greeting && m.body) {
        const across = m.greeting.x + m.greeting.w / 2 - (m.pane.x + m.pane.w / 2)
        const down = m.greeting.y + m.greeting.h / 2 - (m.body.y + m.composer.y) / 2
        if (Math.abs(across) > tolerance || Math.abs(down) > tolerance)
          failures.push(
            `${tag}: the greeting is ${Math.round(across)}px across and ${Math.round(down)}px down from the middle above the composer`,
          )
      }
    }
    const card = (m, tag) => {
      if (!m.pane || !m.composer) return failures.push(`${tag}: no home in a pane`)
      const foot = m.pane.y + m.pane.h - (m.composer.y + m.composer.h)
      if (foot < 120)
        failures.push(
          `${tag}: the card sits ${Math.round(foot)}px from the pane's foot, docked`,
        )
      if (!m.scene) failures.push(`${tag}: no scene in a large pane`)
      if (parseFloat(m.greetingType?.size ?? "0") < 28)
        failures.push(
          `${tag}: the greeting is ${m.greetingType?.size}, not the home's own size`,
        )
    }
    // What a crossing keeps: the same field, its draft, the model, the page
    // or the card — and, where nothing else moved focus, the caret and its place.
    const kept = (m, tag, want) => {
      if (!m.same) failures.push(`${tag}: the composer's field was made anew`)
      if (m.text !== want.text)
        failures.push(`${tag}: the draft reads ${JSON.stringify(m.text)}`)
      if (m.model !== want.model)
        failures.push(`${tag}: the model reads ${m.model}, was ${want.model}`)
      if (m.page !== want.page)
        failures.push(`${tag}: the page is ${m.page ? "open" : "closed"}, was not`)
      if (!want.caret) return
      if (!m.focused) failures.push(`${tag}: the caret left the home's composer`)
      else if (m.caret !== want.caret)
        failures.push(`${tag}: the caret is at ${m.caret}, was at ${want.caret}`)
    }
    const record = async (tag, m) => {
      seen.push({
        tag,
        pane: m.pane && `${Math.round(m.pane.w)}×${Math.round(m.pane.h)}`,
        composerFoot:
          m.pane &&
          m.composer &&
          Math.round(m.pane.y + m.pane.h - m.composer.y - m.composer.h),
        greeting: m.greetingType?.size ?? "off",
        caret: m.focused ? m.caret : "elsewhere",
      })
      await shot(options, page, `home-${engine}-${layout}-${tag}`)
    }
    // Every settling a home starts, from the page's first frame, noted as it
    // starts: only a change of shape may play one, by opacity and transform.
    await page.evaluate(() => {
      window.__homeSettling = []
      document.addEventListener("animationstart", (event) => {
        if (!event.animationName.startsWith("workspace-home")) return
        const animation = event.target
          .getAnimations()
          .find((a) => a.animationName === event.animationName)
        window.__homeSettling.push({
          name: event.animationName,
          properties: (animation?.effect.getKeyframes() ?? []).flatMap((k) =>
            Object.keys(k).filter(
              (p) =>
                ![
                  "offset",
                  "easing",
                  "composite",
                  "computedOffset",
                  "opacity",
                  "transform",
                ].includes(p),
            ),
          ),
        })
      })
    })
    const settling = () =>
      page.evaluate(() => window.__homeSettling.splice(0, window.__homeSettling.length))
    const settles = async (tag, name) => {
      const moved = await settling()
      if (!name) {
        if (moved.length)
          failures.push(`${tag}: plays ${moved.map((m) => m.name).join(", ")}`)
        return
      }
      if (!moved.some((m) => m.name === name))
        failures.push(`${tag}: nothing settles (${name}), it jumps`)
      const laidOut = moved.flatMap((m) => m.properties)
      if (laidOut.length)
        failures.push(`${tag}: it animates ${[...new Set(laidOut)].join(", ")}`)
    }
    const homeField = page.locator(`${css.paneHome} ${css.field}`)
    // The caret put in the draft, `caretAt` along it, by the keyboard.
    const placeCaret = async () => {
      await homeField.click()
      await page.keyboard.press("End")
      for (let i = draft.length; i > caretAt; i--) await page.keyboard.press("ArrowLeft")
    }
    const resize = async (height) => {
      await page.setViewportSize({ width: 1440, height })
      await frames(page, 2)
      await settled(page)
    }

    // A new session's home alone in a large pane is the window's home as it
    // appears, with nothing of the settling played on the way in.
    await page.keyboard.press(keys.newSession)
    await need(page, css.paneHome, "a new session's home")
    await settled(page)
    await settles("appearing alone in a large pane")

    // A conversation, and a new session's home opened beside it: small, and
    // docked as it appears, again with nothing played on the way in.
    const row = page.locator(css.sessionRow).nth(1)
    if (!(await row.count())) throw new CannotRun(`no session row (${css.sessionRow})`)
    await row.click()
    await settled(page)
    await settling()
    await page.keyboard.press(keys.newSessionBeside)
    await paneCountIs(page, 2)
    await need(page, css.paneHome, "a new session's home")
    await settled(page)
    await settles("appearing beside a conversation")
    await homeField.click()
    await page.keyboard.type(draft)
    await page.evaluate((sel) => {
      document.querySelector(`${sel.paneHome} ${sel.field}`).__homeShapeField = true
    }, css)
    // Another model than the one it starts with.
    const first = (await measure()).model
    await page.locator(`${css.paneHome} ${css.modelChip}`).click()
    const other = page.locator(
      `${css.modelPicker} [role="option"][aria-selected="false"]`,
    )
    if (!(await other.count())) throw new CannotRun("no other model to choose")
    await other.first().click()
    await settled(page)
    const small = await measure()
    if (small.model === first) throw new CannotRun(`the model stayed ${first}`)
    docked(small, "beside a conversation")
    await record("small", small)
    const want = { text: draft, model: small.model, page: false, caret: null }

    // The conversation closes: the home takes the whole room, and the card.
    // Closing moves focus by its own key, so the caret is not asked after here.
    await page.keyboard.press(keys.focusPane(1))
    await page.keyboard.press(keys.closePane)
    await paneCountIs(page, 1)
    await frames(page, 2)
    await settles("crossing to the card", "workspace-home-card")
    await settled(page)
    const large = await measure()
    card(large, "alone")
    kept(large, "alone", want)
    await record("large", large)

    // The window shortens under 640px of pane, and grows again: nothing but
    // the pane moves, so the caret stays where it was put, and in the field.
    await placeCaret()
    want.caret = caretAt
    await settling()
    await resize(640)
    await settles("crossing to the dock", "workspace-home-docked")
    const short = await measure()
    docked(short, "a short window")
    kept(short, "a short window", want)
    await record("short", short)
    await resize(900)
    await settles("crossing back to the card", "workspace-home-card")
    const back = await measure()
    card(back, "tall again")
    kept(back, "tall again", want)
    await record("back", back)

    // A long draft opens the page, and a crossing keeps it open.
    await page.keyboard.press("End")
    for (let i = 0; i < 8; i++) await page.keyboard.press("Shift+Enter")
    await settled(page)
    const long = await measure()
    if (!long.page) {
      // Typing that went nowhere is the failure already reported, not the page's.
      if (failures.length) return { shapes: seen, failures }
      throw new CannotRun("a long draft did not open the page")
    }
    const paged = { ...want, text: long.text, page: true, caret: long.caret }
    await resize(640)
    kept(await measure(), "a short window, the page open", paged)
    await resize(900)
    kept(await measure(), "tall again, the page open", paged)

    for (let i = 0; i < 8; i++) await page.keyboard.press("Backspace")
    await settled(page)

    // The composer's opacity on the frame after a crossing: settling, it is
    // still coming in; with less motion, it is simply there.
    const crossing = async (height) => {
      await page.setViewportSize({ width: 1440, height })
      await frames(page, 1)
      const opacity = await page.evaluate(
        (sel) =>
          getComputedStyle(document.querySelector(`${sel.paneHome} ${sel.dock}`)).opacity,
        css,
      )
      await settled(page)
      return Number(opacity)
    }
    const moving = await crossing(640)
    await resize(900)
    if (!(moving < 1))
      failures.push(
        `the frame after a crossing, the composer is already there (${moving})`,
      )
    await page.emulateMedia({ reducedMotion: "reduce" })
    await frames(page, 2)
    const still = await crossing(640)
    if (still !== 1) failures.push(`with less motion, the composer fades in (${still})`)
    await resize(900)
    await page.emulateMedia({ reducedMotion: "no-preference" })
    seen.push({ tag: "opacity the frame after", moving, still })

    // With the greeting off (Settings), the composer still docks at the foot.
    await page.evaluate(
      ([key, event]) => {
        localStorage.setItem(key, "off")
        window.dispatchEvent(new CustomEvent(event, { detail: "off" }))
      },
      [storage.greeting, preferenceEvents.greeting],
    )
    // A conversation in the pane again, and a new home beside it.
    await row.click()
    await settled(page)
    if (await page.locator(css.paneHome).count())
      throw new CannotRun("the session row did not open in the pane")
    await page.keyboard.press(keys.newSessionBeside)
    await paneCountIs(page, 2)
    await need(page, css.paneHome, "a new session's home")
    await settled(page)
    const bare = await measure()
    if (bare.greeting) failures.push("the greeting shows with the greeting off")
    if (!bare.beside) throw new CannotRun("no conversation beside the home")
    docked(bare, "with the greeting off")
    await record("no-greeting", bare)
    if ((await paneCount(page)) !== 2) failures.push("a pane opened or closed on its own")
    return { shapes: seen, failures }
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

/** Where the slider stands and what has the keyboard, by position (the levels are the page's). */
const thinkingState = (page, chip) =>
  page.evaluate(
    ([sel, chipSelector]) => {
      const knob = document.querySelector(sel.thinkingSlider)
      const popover = document.querySelector(sel.thinkingPopover)
      return {
        open: popover !== null,
        count: knob ? Number(knob.getAttribute("aria-valuemax")) + 1 : 0,
        checked: knob ? Number(knob.getAttribute("aria-valuenow")) : -1,
        checkedLabel: knob?.getAttribute("aria-valuetext"),
        knobFocused: knob !== null && document.activeElement === knob,
        ultra:
          document.querySelector(sel.thinkingTrack)?.hasAttribute("data-ultra") ?? false,
        utmost: popover?.hasAttribute("data-utmost") ?? false,
        chipFocused: document.activeElement === document.querySelector(chipSelector),
        chipLabel: document.querySelector(chipSelector)?.getAttribute("aria-label"),
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
    throw new CannotRun(`the slider offers ${opened.count} levels; the walk needs 3`)
  if (!opened.knobFocused)
    failures.push("opened with the keyboard elsewhere than the slider")
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
      failures.push(`${key}: the level is ${now.checked}, not ${want}`)
    if (!now.knobFocused) failures.push(`${key}: the keyboard left the slider`)
    if (now.chipLabel !== `Thinking level: ${now.checkedLabel}`)
      failures.push(`${key}: the chip says "${now.chipLabel}"`)
    // Past Max, where the model has Ultra, is the one level marked apart; below it none is.
    if (now.utmost !== (now.ultra && now.checked === now.count - 1))
      failures.push(
        `${key}: at level ${now.checked} the popover is${now.utmost ? "" : " not"} marked utmost`,
      )
    if (key === keys.end) await shot(options, page, `thinking-${engine}-utmost`)
  }

  // The pointer: pressed near the start and dragged most of the way from the
  // second level to the third, the knob follows it and the level is the
  // nearest; let go, the knob settles on that level's place.
  const track = await page.locator(css.thinkingTrack).boundingBox()
  const midline = track.y + track.height / 2
  const toward = 1.7 / (now.count - 1)
  let held
  await sampling("drag", async () => {
    await page.mouse.move(track.x + 2, midline)
    await page.mouse.down()
    await page.mouse.move(track.x + track.width * toward, midline, { steps: 12 })
    await frames(page, 2)
    const knob = await page.locator(css.thinkingSlider).boundingBox()
    held = {
      pointer: track.x + track.width * toward,
      knob: knob.x + knob.width / 2,
      state: await thinkingState(page, chip),
    }
    await page.mouse.up()
  })
  const dropped = await thinkingState(page, chip)
  const knob = await page.locator(css.thinkingSlider).boundingBox()
  const settledAt = knob.x + knob.width / 2
  const levelAt = track.x + (track.width * 2) / (now.count - 1)
  const drag = {
    followed: Math.round((held.knob - held.pointer) * 100) / 100,
    heldLevel: held.state.checked,
    level: dropped.checked,
    settled: Math.round((settledAt - levelAt) * 100) / 100,
  }
  if (Math.abs(drag.followed) > 2)
    failures.push(`drag: the held knob stood ${drag.followed}px from the pointer`)
  if (drag.heldLevel !== 2 || drag.level !== 2)
    failures.push(
      `drag: the level was ${drag.heldLevel} held and ${drag.level} let go, not 2`,
    )
  if (Math.abs(drag.settled) > 1)
    failures.push(
      `drag: let go, the knob settled ${drag.settled}px from its level's place`,
    )
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
    if (after.checked !== dropped.checked)
      failures.push(`Fast changed the level from ${dropped.checked} to ${after.checked}`)
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

  // The popover stays on its chip when the chip moves with no change of the
  // window's size: a column folding (⌘B), the session list going (⌥⌘S).
  const opens = async () => {
    await page.locator(chip).focus()
    await page.keyboard.press(keys.enter)
    if (!(await until(page, (sel) => document.querySelector(sel), css.thinkingPopover)))
      throw new Error("Return on the chip did not open the thinking popover")
    await settled(page)
  }
  const edges = () =>
    page.evaluate(
      ([sel, chipSelector]) => {
        const c = document.querySelector(chipSelector)?.getBoundingClientRect()
        const p = document.querySelector(sel.thinkingPopover)?.getBoundingClientRect()
        return c && p
          ? { chipRight: c.right, chipTop: c.top, right: p.right, bottom: p.bottom }
          : null
      },
      [css, chip],
    )
  await opens()
  const following = []
  const start = await edges()
  for (const [name, chord] of [
    ["⌘B", keys.toggleSidebar],
    ["⌥⌘S", keys.toggleSessionList],
  ]) {
    await page.keyboard.press(chord)
    await frames(page, 2)
    await settled(page)
    await frames(page, 2)
    const now = await edges()
    if (!now) {
      failures.push(`${name}: the thinking popover closed as its chip moved`)
      break
    }
    const row = {
      name,
      chipMoved: Math.round(now.chipRight - start.chipRight),
      right: Math.round((now.right - now.chipRight) * 100) / 100,
      above: Math.round((now.chipTop - now.bottom) * 100) / 100,
    }
    following.push(row)
    if (Math.abs(row.right) > 1)
      failures.push(`${name}: the popover's edge is ${row.right}px from its chip's`)
    if (Math.abs(row.above - 10) > 1)
      failures.push(`${name}: the popover ends ${row.above}px above its chip, not 10`)
  }
  if (!following.some((row) => row.chipMoved !== 0))
    failures.push(
      "⌘B and ⌥⌘S did not move the chip: nothing showed the popover following",
    )
  await page.keyboard.press(keys.escape)
  await page.keyboard.press(keys.toggleSidebar)
  await page.keyboard.press(keys.toggleSessionList)
  await settled(page)

  // A model whose track ends at Max, and Ultra carried to it: shown as Max,
  // and Max chosen there is Max, not the Ultra carried back.
  const pick = async (model) => {
    await page.locator(`${css.focusedPane} ${css.modelChip}`).click()
    const option = page.locator(css.modelOption, { hasText: model }).first()
    await option.waitFor({ timeout: 3000 })
    await option.click()
    await settled(page)
  }
  await opens()
  await page.keyboard.press(keys.end)
  const carried = await thinkingState(page, chip)
  await page.keyboard.press(keys.escape)
  if (!carried.ultra) throw new CannotRun("the new session's model has no Ultra")
  await pick(names.modelWithoutUltra)
  const shownAs = await thinkingState(page, chip)
  await opens()
  const upToMax = await thinkingState(page, chip)
  if (upToMax.ultra)
    throw new CannotRun(
      `${names.modelWithoutUltra} has Ultra now; name another in selectors.mjs`,
    )
  const noUltra = { count: upToMax.count, carriedShownAs: shownAs.chipLabel }
  if (
    shownAs.chipLabel !== `Thinking level: ${upToMax.checkedLabel}` ||
    upToMax.checked !== upToMax.count - 1
  )
    failures.push(
      `Ultra carried to a model without it shows as "${shownAs.chipLabel}", not its last level`,
    )
  // End there, on the Max already shown, chooses Max: back on the model with
  // Ultra, it is still Max rather than the Ultra carried.
  await page.keyboard.press(keys.end)
  await page.keyboard.press(keys.escape)
  await pick(names.modelWithUltra)
  noUltra.backOnUltra = (await thinkingState(page, chip)).chipLabel
  if (!/Max$/.test(noUltra.backOnUltra ?? ""))
    failures.push(
      `Max chosen on ${names.modelWithoutUltra} came back as "${noUltra.backOnUltra}"`,
    )
  // The walk on a track that ends at Max: it ends there, and nothing is marked apart.
  await pick(names.modelWithoutUltra)
  await opens()
  for (const [key, want] of [
    [keys.home, 0],
    [keys.end, upToMax.count - 1],
  ]) {
    await sampling(`no Ultra: ${key} to ${want}`, () => page.keyboard.press(key))
    const now = await thinkingState(page, chip)
    if (now.checked !== want)
      failures.push(`no Ultra: ${key} left the level at ${now.checked}`)
    if (now.utmost) failures.push(`no Ultra: level ${now.checked} is marked utmost`)
  }
  await page.keyboard.press(keys.escape)

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

  return { opened, moves, drag, following, noUltra, reduced: still, failures }
}

const meta = {
  name: "responsive",
  summary:
    "approval card, composer controls and thinking control, column titles, Settings and a pane's home at many sizes",
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
