#!/usr/bin/env node
/** The details sheet's "Where it runs": each lease state the gateway publishes, in the production sheet. */
import { attempt, CannotRun } from "./lib/cli.mjs"
import { openPage, withEngines } from "./lib/browser.mjs"
import { main } from "./lib/run.mjs"
import { css, leaseRoles } from "./lib/selectors.mjs"

const granted = (status) => [
  ["Computer", "This computer"],
  ["Sandbox", "The agent's own"],
  ["Status", status],
]
/** The rows each published lease shows, in order; `null` is no section at all. */
const expected = {
  none: null,
  live: granted("Allowed to run"),
  ending: granted("Stopping"),
  ended: granted("Ended when Nessa restarted"),
  interrupted: granted("Cleanup not confirmed"),
  // A refused lease names what was asked for, not where anything runs.
  refused: [["Status", "Couldn't start: sandbox not available"]],
  // An unreadable lease claims nothing, so nothing is known of where it runs.
  unreadable: [
    ["Computer", "Not known"],
    ["Sandbox", "Not known"],
    ["Status", "Not known"],
  ],
}

await main(
  {
    name: "lease-details",
    summary: "the details sheet's Where it runs section for every published lease state",
    defaults: { engine: "chromium,webkit" },
  },
  async ({ options, rep, url, mode }) => {
    if (mode === "prod" && !options.given("url"))
      throw new CannotRun(
        "the production-component fixture requires a Vite development server or an explicitly supplied fixture-serving URL",
      )
    await withEngines(options, rep, async (engine, browser) => {
      for (const width of options.quick ? [360] : [360, 600, 1440]) {
        for (const [name, rows] of Object.entries(expected)) {
          const pageUrl = new URL("/verification/conversation/lease-fixture.html", url)
          pageUrl.searchParams.set("case", name)
          const opened = await openPage(browser, {
            url: pageUrl.href,
            readySelector: css.leaseFixture,
            width,
            mac: false,
          })
          try {
            await attempt(rep, { engine, width, name }, async () => {
              const { page } = opened
              // The sheet's title is there before anything is measured.
              await page.getByRole("heading", { name: "Details", exact: true }).waitFor()
              const section = page.getByRole(...leaseRoles.whereItRuns)
              const sections = await section.count()
              const errors = opened.errors.map((line) => `page error: ${line}`)
              if (rows === null)
                return {
                  sections,
                  failures: [
                    ...(sections === 0 ? [] : [`${sections} sections, expected none`]),
                    ...errors,
                  ],
                }
              if (sections !== 1)
                return {
                  sections,
                  failures: [`${sections} sections, expected 1`, ...errors],
                }
              const measured = await section.evaluate((node) => {
                const facts = node.lastElementChild
                return {
                  rows: [...facts.children].map((row) => {
                    const [label, value] = row.children
                    return {
                      label: label.textContent,
                      value: value.textContent,
                      // A value cut short by its ellipsis is not read in full.
                      truncated: value.scrollWidth > value.clientWidth + 1,
                    }
                  }),
                  right: node.getBoundingClientRect().right,
                  viewport: document.documentElement.clientWidth,
                  overflow: node.scrollWidth > node.clientWidth + 1,
                }
              })
              const shown = measured.rows.map(({ label, value }) => [label, value])
              return {
                ...measured,
                failures: [
                  ...(JSON.stringify(shown) !== JSON.stringify(rows)
                    ? [`rows ${JSON.stringify(shown)}, expected ${JSON.stringify(rows)}`]
                    : []),
                  ...measured.rows
                    .filter((row) => row.truncated)
                    .map((row) => `${row.label} is cut short`),
                  ...(measured.overflow ? ["the section overflows horizontally"] : []),
                  ...(measured.right > measured.viewport + 1
                    ? [`the section ends at ${measured.right}, past ${measured.viewport}`]
                    : []),
                  ...errors,
                ],
              }
            })
          } finally {
            await opened.close()
          }
        }
      }
    })
  },
)
