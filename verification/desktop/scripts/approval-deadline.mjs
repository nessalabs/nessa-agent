#!/usr/bin/env node
/**
 * An armed review on the approval card (`fixtures/approval-deadline/`).
 * The pending line names the seconds left, those seconds go down, and at
 * expiry the card is gone and the transcript says the review was denied
 * automatically. Dev server only: the fixture is a page of that server's.
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { approvalDeadline, css } from "./lib/selectors.mjs"
import { settled, until } from "./lib/workspace.mjs"

const meta = {
  name: "approval-deadline",
  summary: "an armed review counts down, then the card is the automatic notice",
  defaults: { engine: "chromium,webkit" },
  help: `
Usage: node verification/desktop/scripts/approval-deadline.mjs [options] [--shots <dir>]

Chromium and WebKit. The deadline ring shows the seconds left (in its last ten,
"Denies in Ns") and is named "If nobody answers within N seconds, this is denied
automatically."; N goes down, and at expiry the card is replaced by
"Denied automatically: …".`,
}

const pending = /^If nobody answers within (\d+) seconds, this is denied automatically\.$/
/** What the ring shows: the seconds, or in its last ten, what it will do. */
const shown = /^(?:Denies in )?(\d+)s$/

const shot = async (options, page, name, locator) => {
  if (!options.shots) return
  mkdirSync(options.shots, { recursive: true })
  await locator.screenshot({ path: join(options.shots, `${name}.png`) })
}

await main(meta, async ({ options, rep, url, mode }) => {
  if (mode === "prod" && !options.given("url"))
    throw new CannotRun(`${approvalDeadline.page} is a dev-server page (--mode dev)`)
  const served = await fetch(new URL(approvalDeadline.page, url)).then(
    (response) => response.ok,
    () => false,
  )
  if (!served)
    throw new CannotRun(
      `${approvalDeadline.page} is not served at ${url}: the fixture is the dev server's (--mode dev)`,
    )
  await withEngines(options, rep, async (engine, browser) => {
    await attempt(rep, { engine, name: "countdown" }, async () => {
      const opened = await openPage(browser, {
        url: new URL(approvalDeadline.page, url).href,
      })
      const { page } = opened
      try {
        const row = page
          .locator(css.sessionRow, { hasText: approvalDeadline.session })
          .first()
        await row.waitFor({ timeout: 10_000 }).catch(() => {})
        if (!(await row.count()))
          throw new CannotRun(`no session row "${approvalDeadline.session}" in the list`)
        await row.click()
        await settled(page)
        const drawn = await until(
          page,
          (sel) => document.querySelector(sel) !== null,
          css.approvalDeadline,
          10_000,
        )
        const ring = page.locator(css.approvalDeadline)
        const firstText = drawn ? ((await ring.getAttribute("aria-label")) ?? "") : ""
        const firstShown = drawn ? (await ring.innerText()).trim() : ""
        const first = pending.exec(firstText)
        const firstSeconds = first ? Number(first[1]) : NaN
        if (drawn)
          await shot(
            options,
            page,
            `${engine}-pending`,
            page.locator(css.approvalCard).first(),
          )
        const decreased = await until(
          page,
          ([sel, started]) => {
            const line = document.querySelector(sel)
            const match = line?.getAttribute("aria-label")?.match(/within (\d+) second/)
            return Boolean(match && Number(match[1]) < started)
          },
          [css.approvalDeadline, firstSeconds],
          8_000,
        )
        const laterText = decreased
          ? ((await ring.getAttribute("aria-label")) ?? "")
          : firstText
        const later = pending.exec(laterText)
        const notice = approvalDeadline.notice(approvalDeadline.reason)
        const replaced = await until(
          page,
          ([card, message, sentence]) =>
            document.querySelector(card) === null &&
            [...document.querySelectorAll(message)].some((el) =>
              el.textContent.includes(sentence),
            ),
          [css.approvalCard, css.message, notice],
          15_000,
        )
        if (replaced)
          await shot(
            options,
            page,
            `${engine}-notice`,
            page.locator(css.message, { hasText: "Denied automatically" }).first(),
          )
        const failures = []
        if (!drawn) failures.push("the deadline row was not drawn")
        else if (!first) failures.push(`pending name was ${JSON.stringify(firstText)}`)
        else if (Number(shown.exec(firstShown)?.[1]) !== firstSeconds)
          failures.push(
            `the ring shows ${JSON.stringify(firstShown)}, not the ${firstSeconds}s its name says`,
          )
        else if (firstSeconds < 2)
          failures.push(
            `countdown started at ${firstSeconds}s, too late to watch it fall`,
          )
        // The fixture's gateway clock is a minute ahead of the page's: a card
        // that measured against its own clock would start near 70 seconds.
        else if (firstSeconds > approvalDeadline.waitSeconds)
          failures.push(
            `countdown started at ${firstSeconds}s, above the ${approvalDeadline.waitSeconds}s wait: the gateway's clock moved it`,
          )
        if (!decreased)
          failures.push(`countdown did not fall from ${JSON.stringify(firstText)}`)
        else if (later && Number(later[1]) >= firstSeconds)
          failures.push(`countdown stayed at ${later[1]}s`)
        if (!replaced)
          failures.push(`the card was not replaced by ${JSON.stringify(notice)}`)
        return {
          pending: firstText,
          later: laterText,
          replaced,
          failures,
        }
      } finally {
        await opened.close()
      }
    })
  })
})
