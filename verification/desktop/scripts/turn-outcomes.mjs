#!/usr/bin/env node
/**
 * Turn endings and questions, in the desktop window and the panel at once
 * (`fixtures/turn-outcomes/`): a failure with no reply, a cancellation after
 * a partial reply, an unresolved turn with no diagnostic, the gateway's
 * notice that a review was left out, and three questions. Answering or
 * skipping on the desktop, or a later frame that no longer lists a question,
 * drops that question on both surfaces.
 *
 * The fixture is a page of the dev server's, not of the production build.
 */
import { mkdirSync } from "node:fs"
import { join } from "node:path"
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { turnOutcomes as ui } from "./lib/selectors.mjs"

const meta = {
  name: "turn-outcomes",
  summary:
    "turn failures, cancellations, unresolved turns, questions, and a withheld-review notice stay visible",
  defaults: { engine: "chromium,webkit" },
  options: { only: { type: "string" } },
  help: `
Usage: node verification/desktop/scripts/turn-outcomes.mjs [options] [--shots <dir>]

Checks, per engine and layout (--only <names> to pick):
  terminals  the desktop shows Failed with "Provider connection lost",
             Cancelled with "Stopped", and Unresolved with no diagnostic;
             "Check the deploy.", "Halfway", and "Wait." stay on both
             surfaces; those turns do not say "Not sent"
  asked      the notice is "A review was left out of this view." and both
             surfaces show the three questions
  cleared    Staging then Answer drops the first question on both surfaces;
             Skip drops the second; a later frame that no longer lists the
             third drops it on both`,
}

const shownWithinMs = 15_000

async function open(browser, url, layout) {
  const opened = await openPage(browser, {
    url: new URL(ui.page, url).href,
    layout,
    width: 1280,
    height: 1400,
    readySelector: `${ui.panel} [data-slot="questionnaire"]`,
  })
  await opened.page
    .locator(`${ui.desktop} ${ui.question("place")}`)
    .waitFor({ timeout: shownWithinMs })
  return opened
}

/** Visible text of one surface. */
const textOf = (page, surface) => page.locator(surface).innerText()

const checks = {
  async terminals({ browser, url, layout, engine, options }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      const marks = await page
        .locator(`${ui.desktop} ${ui.terminal}`)
        .evaluateAll((nodes) =>
          nodes.map((node) => ({
            status: node.getAttribute("data-turn-terminal"),
            error: node.getAttribute("data-turn-error"),
            label: node.querySelector("span")?.textContent ?? "",
          })),
        )
      const expected = [
        { status: "failed", error: "Provider connection lost", label: "Failed" },
        { status: "cancelled", error: "Stopped", label: "Cancelled" },
        { status: "unresolved", error: null, label: "Unresolved" },
      ]
      if (JSON.stringify(marks) !== JSON.stringify(expected))
        failures.push(`desktop marks ${JSON.stringify(marks)}`)
      for (const surface of [ui.desktop, ui.panel]) {
        const text = await textOf(page, surface)
        for (const line of ["Check the deploy.", "Halfway", "Wait."])
          if (!text.includes(line)) failures.push(`${surface} is missing ${line}`)
        if (text.includes("Not sent"))
          failures.push(`${surface} says a provider turn was not sent`)
        if (text.includes("OAuth session expired"))
          failures.push(`${surface} repeats an authentication diagnostic`)
      }
      if (options.shots) {
        mkdirSync(options.shots, { recursive: true })
        await page.locator(`${ui.desktop} .workspace-transcript`).screenshot({
          path: join(options.shots, `turn-outcomes-desktop-${engine}-${layout}.png`),
        })
        await page.locator(ui.panel).screenshot({
          path: join(options.shots, `turn-outcomes-panel-${engine}-${layout}.png`),
        })
      }
      await opened.settleRequests()
      failures.push(...opened.errors)
      return { marks, failures }
    } finally {
      await opened.close()
    }
  },

  async asked({ browser, url, layout }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      const notice = await page.locator(`${ui.desktop} ${ui.notice}`).innerText()
      if (notice !== "A review was left out of this view.")
        failures.push(`notice says "${notice}"`)
      for (const surface of [ui.desktop, ui.panel]) {
        const text = await textOf(page, surface)
        for (const line of [
          "Where should this land?",
          "When should it ship?",
          "Who should hear?",
        ])
          if (!text.includes(line)) failures.push(`${surface} is missing ${line}`)
      }
      await opened.settleRequests()
      failures.push(...opened.errors)
      return { notice, failures }
    } finally {
      await opened.close()
    }
  },

  async cleared({ browser, url, layout }) {
    const opened = await open(browser, url, layout)
    const failures = []
    try {
      const { page } = opened
      const place = page.locator(`${ui.desktop} ${ui.question("place")}`)
      await place.locator(ui.choice, { hasText: "Staging" }).click()
      await place.locator("button", { hasText: "Answer" }).click()
      await place.waitFor({ state: "detached", timeout: shownWithinMs })
      const when = page.locator(`${ui.desktop} ${ui.question("when")}`)
      await when.locator("button", { hasText: "Skip" }).click()
      await when.waitFor({ state: "detached", timeout: shownWithinMs })
      await page.evaluate(() => window.__outcomes.forget("elsewhere"))
      await page
        .locator(`${ui.desktop} ${ui.question("elsewhere")}`)
        .waitFor({ state: "detached", timeout: shownWithinMs })
      for (const surface of [ui.desktop, ui.panel]) {
        const text = await textOf(page, surface)
        for (const line of [
          "Where should this land?",
          "When should it ship?",
          "Who should hear?",
        ])
          if (text.includes(line)) failures.push(`${surface} still shows ${line}`)
      }
      const kept = await textOf(page, ui.desktop)
      if (!kept.includes("Provider connection lost") || !kept.includes("Halfway"))
        failures.push("answering a question dropped the turn outcomes")
      await opened.settleRequests()
      failures.push(...opened.errors)
      return { failures }
    } finally {
      await opened.close()
    }
  },
}

await main(meta, async ({ options, rep, url }) => {
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
