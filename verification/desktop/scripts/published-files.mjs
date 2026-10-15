#!/usr/bin/env node
/**
 * The files an agent published (slice D2b), in the desktop window and the
 * panel at once (`fixtures/published-files/`): each card sits inside its
 * transcript's column; a picture is read back through the real client,
 * verified, and painted at its natural size; every window reads at most two
 * files at once; a file the conversation no longer keeps says so, and one it
 * could not read now offers Retry and shows once retried; a save in a browser
 * downloads the file byte for byte, and a save in the desktop app goes to the
 * host's own save commands in bounded parts with no `a[download]` in the
 * page; and a conversation with no turn, only a file published while none
 * ran, shows that file rather than the empty welcome.
 *
 * The fixture is a page of the dev server's, not of the production build:
 * this script runs against the dev server only.
 */
import { mkdirSync } from "node:fs"
import { readFile } from "node:fs/promises"
import { join } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { need, openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, publishedFiles as ui } from "./lib/selectors.mjs"

const meta = {
  name: "published-files",
  summary:
    "published files: placed in the column, painted verified, read two at a time, saved exactly",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/published-files.mjs [options] [--shots <dir>]

Checks, per engine and layout (--only <names> to pick):
  layout     every card, in the desktop window and the panel, is inside its
             transcript's column (left and right within 0.5 px); noise.png
             (160 × 120) is complete, decoded at 160 × 120 and painted at
             160 × 120 CSS px
  queue      every held picture ends shown on both surfaces, and neither
             surface had more than 2 reads at the gateway at once
  states     gone.png says "No longer kept" and offers no Retry; flaky.png
             says "Couldn’t load" with a Retry, and once the fixture can read
             it, Retry paints it
  save       in the browser: Save notes.txt, noise.png and large.bin each
             download the published bytes exactly, under the published name, and no
             a[download] stays in the page; saving gone.png says "Couldn’t
             save"
  save-host  with the desktop host stood in for (__TAURI_INTERNALS__): Save
             noise.png and Save large.bin each begin one host save, send
             every part at most 1 MiB (large.bin as 1 MiB, 1 MiB, 0.5 MiB + 3),
             finish with the published bytes exactly, give nothing up, and
             the page neither downloads nor ever holds an a[download]
  empty      a panel conversation with no turn and one file published while
             none ran shows that file's card and not the empty welcome`,
}

const PART_BYTES = 1024 * 1024
/** The fixture's files on each surface's turn. */
const cardsEach = 15
/** The parts a host save of `large.bin` (2.5 MiB and 3 bytes) is sent in. */
const largeParts = [PART_BYTES, PART_BYTES, PART_BYTES / 2 + 3]
const drawnWithinMs = 15_000

/** The fixture's page, open in one engine and layout, with the stand-in host when asked. */
async function open(browser, url, layout, initScripts = []) {
  return openPage(browser, {
    url: new URL(ui.page, url).href,
    layout,
    width: 1280,
    height: 1400,
    readySelector: ui.card("noise.png"),
    initScripts,
  })
}

const stats = (page) => page.evaluate(() => window.__published.stats())
const bytesOf = (page, name) =>
  page.evaluate((name) => window.__published.bytesOf(name), name)

/** Every held picture shown in `surface`, or what was not, after `drawnWithinMs`. */
async function allShown(page, surface) {
  return page.evaluate(
    async ({ surface, names, shown, within }) => {
      const deadline = performance.now() + within
      const missing = () =>
        names.filter(
          (name) =>
            !document.querySelector(
              `${surface} [data-published-file="${name}"] ${shown}`,
            ),
        )
      while (missing().length && performance.now() < deadline)
        await new Promise((resolve) => setTimeout(resolve, 50))
      return missing()
    },
    { surface, names: ui.pictures, shown: ui.shownPicture, within: drawnWithinMs },
  )
}

const checks = {
  async layout({ browser, url, layout, engine, options }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      await need(page, css.transcript, "the desktop transcript")
      for (const surface of [ui.desktop, ui.panel]) {
        const missing = await allShown(page, surface)
        if (missing.length) failures.push(`${surface}: not shown: ${missing.join(", ")}`)
      }
      const placed = await page.evaluate(
        ({ desktop, panel, transcript }) => {
          const within = (surface, column) =>
            [...document.querySelectorAll(`${surface} [data-published-file]`)].map(
              (card) => {
                const box = card.getBoundingClientRect()
                const bounds = column.getBoundingClientRect()
                return {
                  surface,
                  name: card.getAttribute("data-published-file"),
                  left: box.left - bounds.left,
                  right: bounds.right - box.right,
                  width: box.width,
                }
              },
            )
          const desktopColumn = document.querySelector(`${desktop} ${transcript}`)
          const panelColumn = document.querySelector(panel)
          return [
            ...(desktopColumn ? within(desktop, desktopColumn) : []),
            ...(panelColumn ? within(panel, panelColumn) : []),
          ]
        },
        { desktop: ui.desktop, panel: ui.panel, transcript: css.transcript },
      )
      if (placed.length !== 2 * cardsEach)
        failures.push(`expected ${cardsEach} cards on each surface, saw ${placed.length}`)
      for (const card of placed)
        if (card.left < -0.5 || card.right < -0.5 || card.width <= 0)
          failures.push(
            `${card.surface} ${card.name} leaves its column: left ${card.left.toFixed(1)}, right ${card.right.toFixed(1)}`,
          )
      const pictures = await page.evaluate(
        ({ surfaces, card, shown }) =>
          surfaces.map((surface) => {
            const image = document.querySelector(`${surface} ${card} ${shown}`)
            if (!image) return { surface, found: false }
            const box = image.getBoundingClientRect()
            return {
              surface,
              found: true,
              complete: image.complete,
              natural: [image.naturalWidth, image.naturalHeight],
              painted: [box.width, box.height],
              src: image.currentSrc.slice(0, 5),
            }
          }),
        {
          surfaces: [ui.desktop, ui.panel],
          card: ui.card("noise.png"),
          shown: ui.shownPicture,
        },
      )
      const expected = await page.evaluate(() => window.__published.picture)
      for (const picture of pictures) {
        if (!picture.found) {
          failures.push(`${picture.surface}: noise.png has no picture`)
          continue
        }
        if (!picture.complete) failures.push(`${picture.surface}: noise.png not decoded`)
        if (picture.src !== "blob:")
          failures.push(`${picture.surface}: noise.png painted from ${picture.src}`)
        if (
          picture.natural[0] !== expected.width ||
          picture.natural[1] !== expected.height
        )
          failures.push(
            `${picture.surface}: noise.png decoded at ${picture.natural.join(" × ")}`,
          )
        if (
          Math.abs(picture.painted[0] - expected.width) > 0.5 ||
          Math.abs(picture.painted[1] - expected.height) > 0.5
        )
          failures.push(
            `${picture.surface}: noise.png painted at ${picture.painted.map((n) => n.toFixed(1)).join(" × ")}`,
          )
      }
      if (options.shots) {
        mkdirSync(options.shots, { recursive: true })
        await page
          .locator(css.transcript)
          .first()
          .screenshot({
            path: join(options.shots, `published-files-desktop-${engine}-${layout}.png`),
          })
        await page.locator("#panels").screenshot({
          path: join(options.shots, `published-files-panel-${engine}-${layout}.png`),
        })
      }
      await opened.settleRequests()
      return { cards: placed.length, pictures, failures }
    } finally {
      await opened.close()
    }
  },

  async queue({ browser, url, layout }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      for (const surface of [ui.desktop, ui.panel]) {
        const missing = await allShown(page, surface)
        if (missing.length) failures.push(`${surface}: not shown: ${missing.join(", ")}`)
      }
      const seen = await stats(page)
      for (const surface of ["desktop", "panel"]) {
        const peak = seen.peaks[surface] ?? 0
        if (peak < 1) failures.push(`${surface} read nothing`)
        if (peak > 2) failures.push(`${surface} had ${peak} reads at the gateway at once`)
      }
      await opened.settleRequests()
      return { ...seen, failures }
    } finally {
      await opened.close()
    }
  },

  async states({ browser, url, layout, engine, options }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      const desktop = page.locator(ui.desktop)
      const gone = desktop.locator(ui.card("gone.png"))
      await gone
        .getByText(ui.notKept, { exact: true })
        .waitFor({ timeout: drawnWithinMs })
      if (await gone.getByRole("button", { name: ui.retry, exact: true }).count())
        failures.push("gone.png offers a Retry for an answer that holds")
      const flaky = desktop.locator(ui.card("flaky.png"))
      await flaky
        .getByText(ui.notLoaded, { exact: true })
        .waitFor({ timeout: drawnWithinMs })
      const retry = flaky.getByRole("button", { name: ui.retry, exact: true })
      if ((await retry.count()) !== 1) failures.push("flaky.png offers no Retry")
      if (options.shots) {
        mkdirSync(options.shots, { recursive: true })
        await flaky.screenshot({
          path: join(options.shots, `published-files-retry-${engine}-${layout}.png`),
        })
        await gone.screenshot({
          path: join(options.shots, `published-files-not-kept-${engine}-${layout}.png`),
        })
      }
      await page.evaluate(() => window.__published.allowFlaky())
      await retry.click()
      await flaky.locator(ui.shownPicture).waitFor({ timeout: drawnWithinMs })
      await opened.settleRequests()
      return { failures }
    } finally {
      await opened.close()
    }
  },

  async save({ browser, url, layout }) {
    const opened = await open(browser, url, layout)
    const failures = []
    const saved = []
    try {
      const { page } = opened
      const desktop = page.locator(ui.desktop)
      for (const name of ["notes.txt", "noise.png", "large.bin"]) {
        const [download] = await Promise.all([
          page.waitForEvent("download", { timeout: drawnWithinMs }),
          desktop.locator(ui.save(name)).click(),
        ])
        const bytes = await readFile(await download.path())
        const expected = Buffer.from(await bytesOf(page, name), "base64")
        saved.push({ name, as: download.suggestedFilename(), bytes: bytes.length })
        if (download.suggestedFilename() !== name)
          failures.push(`${name} downloaded as ${download.suggestedFilename()}`)
        if (!bytes.equals(expected))
          failures.push(
            `${name}: ${bytes.length} bytes downloaded, not the ${expected.length} published`,
          )
      }
      if (await page.locator(ui.download).count())
        failures.push("an a[download] stayed in the page")
      const gone = desktop.locator(ui.card("gone.png"))
      await gone
        .getByText(ui.notKept, { exact: true })
        .waitFor({ timeout: drawnWithinMs })
      await gone.locator(ui.save("gone.png")).click()
      await gone
        .locator(ui.status)
        .getByText(ui.notSaved, { exact: true })
        .waitFor({ timeout: drawnWithinMs })
      await opened.settleRequests()
      return { saved, failures }
    } finally {
      await opened.close()
    }
  },

  async "save-host"({ browser, url, layout }) {
    // The desktop app's host, stood in for before the page loads: its save
    // commands keep what they are sent the way `saving::sessions` does, and
    // every a[download] the page ever inserts is counted.
    const host = () => {
      const saves = { begun: [], parts: {}, finished: {}, aborted: [], anchors: 0 }
      const open = {}
      let callbacks = 0
      window.__hostSaves = saves
      window.__TAURI_INTERNALS__ = {
        transformCallback: () => ++callbacks,
        invoke: async (command, args, options) => {
          if (command === "save_file_begin") {
            saves.begun.push(args.name)
            const token = `t${saves.begun.length}`
            open[token] = []
            saves.parts[token] = []
            return { state: "open", token }
          }
          if (command === "save_file_part") {
            const token = options?.headers?.["x-nessa-save-token"]
            const bytes = args instanceof Uint8Array ? args : new Uint8Array(args)
            if (!open[token]) throw { reason: "not-open", detail: "no such save" }
            saves.parts[token].push(bytes.byteLength)
            open[token].push(bytes.slice())
            return null
          }
          if (command === "save_file_finish") {
            const chunks = open[args.token]
            delete open[args.token]
            let text = ""
            for (const chunk of chunks)
              for (const byte of chunk) text += String.fromCharCode(byte)
            saves.finished[args.token] = btoa(text)
            return null
          }
          if (command === "save_file_abort") {
            delete open[args.token]
            saves.aborted.push(args.token)
            return null
          }
          return null
        },
      }
      new MutationObserver((records) => {
        for (const record of records)
          for (const node of record.addedNodes)
            if (
              node instanceof Element &&
              (node.matches("a[download]") || node.querySelector("a[download]"))
            )
              saves.anchors++
      }).observe(document, { childList: true, subtree: true })
    }
    const opened = await open(browser, url, layout, [host])
    const failures = []
    try {
      const { page } = opened
      let downloads = 0
      page.on("download", () => downloads++)
      await page
        .locator(ui.desktop)
        .locator(ui.card("noise.png"))
        .locator(ui.shownPicture)
        .waitFor({
          timeout: drawnWithinMs,
        })
      const desktop = page.locator(ui.desktop)
      const names = ["noise.png", "large.bin"]
      for (const [index, name] of names.entries()) {
        await desktop.locator(ui.save(name)).click()
        await page.waitForFunction(
          (token) => Object.hasOwn(window.__hostSaves.finished, token),
          `t${index + 1}`,
          { timeout: drawnWithinMs },
        )
      }
      const saves = await page.evaluate(() => window.__hostSaves)
      if (JSON.stringify(saves.begun) !== JSON.stringify(names))
        failures.push(`the host was asked to save ${JSON.stringify(saves.begun)}`)
      for (const [index, name] of names.entries()) {
        const token = `t${index + 1}`
        const parts = saves.parts[token] ?? []
        if (parts.some((size) => size > PART_BYTES || size <= 0))
          failures.push(
            `${name}: a part outside 1..${PART_BYTES} bytes: ${parts.join(", ")}`,
          )
        if (saves.finished[token] !== (await bytesOf(page, name)))
          failures.push(`${name}: the host's file is not the published bytes`)
      }
      if (JSON.stringify(saves.parts.t2) !== JSON.stringify(largeParts))
        failures.push(
          `large.bin sent as parts ${saves.parts.t2?.join(", ")}, not ${largeParts.join(", ")}`,
        )
      if (saves.aborted.length)
        failures.push(`saves given up: ${saves.aborted.join(", ")}`)
      if (downloads) failures.push(`${downloads} downloads started in the page`)
      if (saves.anchors)
        failures.push(`${saves.anchors} a[download] inserted into the page`)
      await opened.settleRequests()
      return { parts: saves.parts, failures }
    } finally {
      await opened.close()
    }
  },

  async empty({ browser, url, layout }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      const loose = page.locator(ui.loosePanel)
      await loose.locator(ui.card("loose.txt")).waitFor({ timeout: drawnWithinMs })
      if (await loose.getByText(ui.welcome, { exact: true }).count())
        failures.push("the empty welcome shows beside a published file")
      await opened.settleRequests()
      return { failures }
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
