#!/usr/bin/env node
/**
 * Where an agent runs, as a person sees it: the details sheet's "Where it
 * runs" for each lease state the gateway publishes, on this computer or an
 * SSH host, and the composer tray's "Run on" choice of host for a new
 * conversation, both production components.
 */
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
  // On an SSH host, the host is named in full in place of the computer.
  "ssh-live": [
    ["SSH host", "me@build-01.internal.example.com"],
    ["Sandbox", "The agent's own"],
    ["Status", "Allowed to run"],
  ],
  "ssh-lost": [
    ["SSH host", "devbox"],
    ["Sandbox", "The agent's own"],
    ["Status", "Ended when the connection to its host was lost"],
  ],
  // Refused, it still names the host it was asked of: that is what to fix.
  "ssh-refused": [
    ["SSH host", "devbox"],
    ["Status", "Couldn't start: host not reachable"],
  ],
}

/**
 * The composer tray's "Run on": offered only when the gateway names a host,
 * listing this computer and each host in full, and showing the one chosen.
 */
async function runOn(browser, rep, url, engine, width) {
  for (const name of ["none", "hosts"]) {
    const pageUrl = new URL("/verification/conversation/run-on-fixture.html", url)
    pageUrl.searchParams.set("case", name)
    const opened = await openPage(browser, {
      url: pageUrl.href,
      readySelector: css.runOnFixture,
      width,
      mac: false,
    })
    try {
      await attempt(rep, { engine, width, name: `run-on-${name}` }, async () => {
        const { page } = opened
        const failures = opened.errors.map((line) => `page error: ${line}`)
        const plus = page.getByRole("button", { name: /More options|Add attachment/ })
        await plus.click()
        const row = page.getByRole("button", { name: /^Run on/ })
        if (name === "none") {
          const rows = await row.count()
          return {
            rows,
            failures: [...(rows ? ["Run on is offered with no host"] : []), ...failures],
          }
        }
        const before = (await row.textContent())?.trim()
        if (before !== "Run onThis computer")
          failures.push(`the row reads ${JSON.stringify(before)} before a choice`)
        await row.click()
        const group = page.getByRole("radiogroup", { name: "Run on", exact: true })
        const radios = await group.getByRole("radio").evaluateAll((nodes) =>
          nodes.map((node) => {
            const name = node.firstElementChild?.firstElementChild
            return {
              text: node.textContent,
              checked: node.getAttribute("aria-checked"),
              focused: node === document.activeElement,
              // A host cut short is not read in full.
              cut: name ? name.scrollWidth > name.clientWidth + 1 : true,
              right: node.getBoundingClientRect().right,
            }
          }),
        )
        const texts = radios.map((radio) => radio.text)
        const wanted = [
          "This computer",
          "devboxSSH",
          "me@build-01.internal.example.comSSH",
        ]
        if (JSON.stringify(texts) !== JSON.stringify(wanted))
          failures.push(
            `choices ${JSON.stringify(texts)}, expected ${JSON.stringify(wanted)}`,
          )
        if (radios[0]?.checked !== "true" || !radios[0]?.focused)
          failures.push("this computer is not the checked, focused choice")
        const viewport = await page.evaluate(() => document.documentElement.clientWidth)
        for (const radio of radios) {
          if (radio.cut) failures.push(`${radio.text} is cut short`)
          if (radio.right > viewport + 1)
            failures.push(`${radio.text} ends at ${radio.right}, past ${viewport}`)
        }
        await group.getByRole("radio", { name: /^me@build-01/ }).click()
        const chosen = await page.locator(css.runOnFixture).getAttribute("data-chosen")
        if (chosen !== "me@build-01.internal.example.com")
          failures.push(`chose ${JSON.stringify(chosen)}`)
        const after = (await row.textContent())?.trim()
        if (after !== "Run onme@build-01.internal.example.com")
          failures.push(`the row reads ${JSON.stringify(after)} after a choice`)
        const overflow = await page.evaluate(
          () =>
            document.documentElement.scrollWidth > document.documentElement.clientWidth,
        )
        if (overflow) failures.push("the page scrolls horizontally")
        return { texts, chosen, after, failures }
      })
    } finally {
      await opened.close()
    }
  }
}

await main(
  {
    name: "lease-details",
    summary:
      "the details sheet's Where it runs section for every published lease state, here or on an SSH host, and the composer's Run on choice",
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
        await runOn(browser, rep, url, engine, width)
      }
    })
  },
)
