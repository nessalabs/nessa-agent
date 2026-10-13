#!/usr/bin/env node
/**
 * Notes left in a conversation, in the desktop window and the panel at once
 * (`fixtures/transcript-notes/`): each note is a row of its own — inside no
 * chat bubble and no turn — right after the turn it followed, one left before
 * any turn or after a turn the bounded view left out first, and one left
 * between two inputs still waiting right after the first of them; each says
 * who left it, from where, and where it stands, in the same words on both
 * surfaces; and a note whose text is one very long word wraps inside its row,
 * leaving nothing wider than its column.
 *
 * The fixture is a page of the dev server's, not of the production build:
 * this script runs against the dev server only.
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, transcriptNotes as ui } from "./lib/selectors.mjs"

const meta = {
  name: "transcript-notes",
  summary:
    "transcript notes: own rows after their turn, labelled by author and state, long text wrapped",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/transcript-notes.mjs [options] [--shots <dir>]

Checks, per engine and layout (--only <names> to pick):
  placement  on both surfaces, notes and what the turns say read, top to
             bottom: before, dropped, Plan the release., Here is the plan.,
             assigned, expired, Ship it., Shipped., pending, long, Tag the
             release., lost, Announce it. — a note left before any turn or
             after a turn the view left out first, each other right after its
             turn, and one left between the two waiting inputs right after
             the first; no note is inside a chat bubble or a turn's reply
  labels     each note's author line and state, identical on both surfaces:
             "Note left by the owner, from desktop" or "Note left by someone it
             is shared with, from phone"; no state for a recorded note,
             "Waiting for the next turn", "Given to a turn", "Not
             delivered: expired", "Not delivered: sharing changed"
  wrap       every note's row lies inside its transcript's column (within
             0.5 px) and is no wider than its own box (scrollWidth ≤
             clientWidth + 1); the long note's text runs over more than two
             lines; neither column nor the page scrolls sideways`,
}

const shownWithinMs = 15_000

/** What each note's row says: its author line and its state, `null` for none. */
const ownerLine = "Note left by the owner, from desktop"
const sharedLine = "Note left by someone it is shared with, from phone"
const expectedLabels = {
  before: [ownerLine, null],
  dropped: [ownerLine, null],
  assigned: [ownerLine, "Given to a turn"],
  expired: [sharedLine, "Not delivered: expired"],
  pending: [sharedLine, "Waiting for the next turn"],
  long: [sharedLine, null],
  lost: [sharedLine, "Not delivered: sharing changed"],
}

/** The fixture's page, open in one engine and layout, once both surfaces show their notes. */
async function open(browser, url, layout) {
  const opened = await openPage(browser, {
    url: new URL(ui.page, url).href,
    layout,
    width: 1280,
    height: 1500,
    readySelector: `${ui.panel} ${ui.note("lost")}`,
  })
  await opened.page
    .locator(`${ui.desktop} ${ui.note("lost")}`)
    .waitFor({ timeout: shownWithinMs })
  return opened
}

/** Notes and what the turns say on one surface, top to bottom. */
const readingOrder = (page, surface) =>
  page.evaluate(
    ({ surface, anyNote, said }) =>
      [...document.querySelectorAll(`${surface} :is(${anyNote}, ${said})`)].map(
        (element) =>
          element.hasAttribute("data-note")
            ? element.getAttribute("data-note")
            : (element.textContent ?? "").trim(),
      ),
    { surface, anyNote: ui.anyNote, said: ui.said },
  )

const checks = {
  async placement({ browser, url, layout }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      const expected = await page.evaluate(() => window.__notes.order)
      const seen = {}
      for (const surface of [ui.desktop, ui.panel]) {
        const order = await readingOrder(page, surface)
        seen[surface] = order
        if (JSON.stringify(order) !== JSON.stringify(expected))
          failures.push(
            `${surface} reads ${order.join(" | ")}, not ${expected.join(" | ")}`,
          )
      }
      const inside = await page.evaluate(
        ({ anyNote, said }) =>
          [...document.querySelectorAll(anyNote)]
            .filter((note) => note.parentElement?.closest(said))
            .map((note) => note.getAttribute("data-note")),
        { anyNote: ui.anyNote, said: ui.said },
      )
      if (inside.length)
        failures.push(`notes inside a bubble or reply: ${inside.join(", ")}`)
      await opened.settleRequests()
      failures.push(...opened.errors)
      return { order: seen, failures }
    } finally {
      await opened.close()
    }
  },

  async labels({ browser, url, layout }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      const read = await page.evaluate(
        ({ surfaces, ids, note, label, status }) =>
          surfaces.flatMap((surface) =>
            ids.map((id) => {
              const row = document.querySelector(`${surface} ${note.replace("%", id)}`)
              return {
                surface,
                id,
                found: Boolean(row),
                label: row?.querySelector(label)?.textContent ?? null,
                status: row?.querySelector(status)?.textContent ?? null,
              }
            }),
          ),
        {
          surfaces: [ui.desktop, ui.panel],
          ids: Object.keys(expectedLabels),
          note: ui.note("%"),
          label: ui.label,
          status: ui.status,
        },
      )
      for (const each of read) {
        const [label, status] = expectedLabels[each.id]
        if (!each.found) failures.push(`${each.surface} ${each.id}: no row`)
        else {
          if (each.label !== label)
            failures.push(
              `${each.surface} ${each.id}: says "${each.label}", not "${label}"`,
            )
          if (each.status !== status)
            failures.push(
              `${each.surface} ${each.id}: state "${each.status}", not "${status}"`,
            )
        }
      }
      await opened.settleRequests()
      failures.push(...opened.errors)
      return { read, failures }
    } finally {
      await opened.close()
    }
  },

  async wrap({ browser, url, layout, engine, options }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      const measured = await page.evaluate(
        ({ desktop, panel, transcript, anyNote, text, long }) => {
          const columns = [
            [desktop, document.querySelector(`${desktop} ${transcript}`)],
            [panel, document.querySelector(panel)],
          ]
          const rows = columns.flatMap(([surface, column]) =>
            column
              ? [...column.querySelectorAll(anyNote)].map((row) => {
                  const box = row.getBoundingClientRect()
                  const bounds = column.getBoundingClientRect()
                  return {
                    surface,
                    id: row.getAttribute("data-note"),
                    left: box.left - bounds.left,
                    right: bounds.right - box.right,
                    width: box.width,
                    overflow: row.scrollWidth - row.clientWidth,
                  }
                })
              : [],
          )
          const lines = columns.map(([surface, column]) => {
            const words = column?.querySelector(`${long} ${text}`)
            if (!words) return { surface, lines: 0 }
            const lineHeight = parseFloat(getComputedStyle(words).lineHeight)
            return { surface, lines: words.getBoundingClientRect().height / lineHeight }
          })
          const sideways = columns.map(([surface, column]) => ({
            surface,
            overflow: column ? column.scrollWidth - column.clientWidth : null,
          }))
          return {
            rows,
            lines,
            sideways,
            page: document.documentElement.scrollWidth - window.innerWidth,
          }
        },
        {
          desktop: ui.desktop,
          panel: ui.panel,
          transcript: css.transcript,
          anyNote: ui.anyNote,
          text: ui.text,
          long: ui.note("long"),
        },
      )
      if (measured.rows.length !== 2 * Object.keys(expectedLabels).length)
        failures.push(
          `expected ${Object.keys(expectedLabels).length} notes on each surface, measured ${measured.rows.length}`,
        )
      for (const row of measured.rows) {
        if (row.left < -0.5 || row.right < -0.5 || row.width <= 0)
          failures.push(
            `${row.surface} ${row.id} leaves its column: left ${row.left.toFixed(1)}, right ${row.right.toFixed(1)}`,
          )
        if (row.overflow > 1)
          failures.push(
            `${row.surface} ${row.id} is ${row.overflow}px wider than its box`,
          )
      }
      for (const each of measured.lines)
        if (!(each.lines > 2))
          failures.push(
            `${each.surface}: the long note runs over ${each.lines.toFixed(1)} lines`,
          )
      for (const each of measured.sideways)
        if (each.overflow === null || each.overflow > 1)
          failures.push(
            `${each.surface}: the column scrolls sideways by ${each.overflow}px`,
          )
      if (measured.page > 1)
        failures.push(`the page scrolls sideways by ${measured.page}px`)
      if (options.shots) {
        mkdirSync(options.shots, { recursive: true })
        await page.locator(`${ui.desktop} ${css.transcript}`).screenshot({
          path: join(options.shots, `transcript-notes-desktop-${engine}-${layout}.png`),
        })
        await page.locator(ui.panel).screenshot({
          path: join(options.shots, `transcript-notes-panel-${engine}-${layout}.png`),
        })
      }
      await opened.settleRequests()
      failures.push(...opened.errors)
      return { ...measured, failures }
    } finally {
      await opened.close()
    }
  },
}

await main(meta, async ({ options, rep, url }) => {
  // A production build has no such page and answers any path with its own.
  const page = await fetch(new URL(ui.page, url)).then(
    (response) => (response.ok ? response.text() : ""),
    () => "",
  )
  if (!page.includes(`<title>${ui.title}</title>`))
    throw new CannotRun(
      `${ui.page} is not served at ${url}: the fixture is the dev server's (--mode dev)`,
    )
  const only = options.only ? options.only.split(",") : null
  await withEngines(options, rep, async (engine, browser) => {
    for (const layout of options.layouts)
      for (const [name, check] of Object.entries(checks)) {
        if (only && !only.includes(name)) continue
        await attempt(rep, { name, engine, layout }, () =>
          check({ browser, url, layout, engine, options }),
        )
      }
  })
})
