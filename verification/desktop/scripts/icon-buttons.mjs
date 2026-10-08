#!/usr/bin/env node
/**
 * The window's icon buttons (#632): one component (`ui/icon-button.tsx`) whose
 * size is 26, 28 or 32, whose shape is rounded or a pill, and whose name,
 * tooltip and hover are said one way. Every icon button on the page is held
 * to that, and none of the classes it replaced is left.
 */
import { openPage, need, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css } from "./lib/selectors.mjs"

const sizes = { sm: 26, md: 28, lg: 32 }

await main(
  {
    name: "icon-buttons",
    summary: "every icon button: size, shape, name, tooltip, hover, keyboard focus",
    defaults: { engine: "chromium,webkit" },
    help: `
Checks, per engine and layout:
  contract   each .desktop-icon-button is square at its size (26/28/32), a pill is
             fully round and a rounded one is not, its aria-label is its tooltip
             text plus its shortcut in parentheses, and none of the replaced
             classes (workspace-icon-button, desktop-titlebar-button,
             desktop-footer-button, desktop-header-tool) is on the page
  hover      the pointer over each enabled one fills it with the window's stronger hover
  focus      keyboard focus on each enabled one draws an outline

Only what is mounted on load is covered: the titlebar, pane actions, session\n  list and the sidebar's footer. The header picture's zoom tools mount while a
  picture is being adjusted, and are covered by responsive.mjs --only header-picture.`,
  },
  async ({ options, rep, url }) => {
    await withEngines(options, rep, async (engine, browser) => {
      for (const layout of options.layouts) {
        await attempt(rep, { name: "contract", engine, layout }, async () => {
          const { page, close } = await openPage(browser, { url, layout })
          try {
            await need(page, css.anyReady, "the window")
            await need(page, ".desktop-icon-button", "an icon button")
            const found = await page.evaluate((expected) => {
              const failures = []
              const buttons = [...document.querySelectorAll(".desktop-icon-button")]
              for (const button of buttons) {
                const box = button.getBoundingClientRect()
                if (box.width === 0 && box.height === 0) continue
                const name = button.getAttribute("aria-label") ?? ""
                const want = expected[button.dataset.size]
                if (want === undefined) {
                  failures.push(`${name}: unknown size ${button.dataset.size}`)
                  continue
                }
                if (!["rounded", "pill"].includes(button.dataset.shape)) {
                  failures.push(`${name}: unknown shape ${button.dataset.shape}`)
                  continue
                }
                if (Math.abs(box.width - want) > 0.5 || Math.abs(box.height - want) > 0.5)
                  failures.push(
                    `${name}: ${box.width}x${box.height}, not ${want} (${button.dataset.size})`,
                  )
                const radius = parseFloat(getComputedStyle(button).borderTopLeftRadius)
                const round = radius >= box.width / 2 - 0.5
                if ((button.dataset.shape === "pill") !== round)
                  failures.push(`${name}: ${button.dataset.shape} with radius ${radius}`)
                const text = button.dataset.tooltip
                const chord = button.dataset.tooltipShortcut
                // A disabled one says why it is unavailable instead.
                const named = chord ? `${text} (${chord})` : text
                if (!button.disabled && name !== named)
                  failures.push(
                    `${name || "(unnamed)"}: tooltip ${text}, shortcut ${chord}`,
                  )
              }
              const legacy = document.querySelectorAll(
                ".workspace-icon-button, .desktop-titlebar-button, .desktop-footer-button, .desktop-header-tool",
              ).length
              if (legacy > 0)
                failures.push(`${legacy} buttons still use a replaced class`)
              return { failures, buttons: buttons.length }
            }, sizes)
            return { failures: found.failures, measured: { buttons: found.buttons } }
          } finally {
            await close().catch(() => {})
          }
        })

        await attempt(rep, { name: "hover", engine, layout }, async () => {
          const { page, close } = await openPage(browser, { url, layout })
          try {
            await need(page, ".desktop-icon-button:visible", "an icon button")
            // A resting button (Back/Forward with no history) answers nothing, on purpose.
            const buttons = page.locator(
              ".desktop-icon-button:visible:not(:disabled):not([aria-disabled]):not([aria-hidden=true])",
            )
            const count = await buttons.count()
            const failures = []
            const seen = []
            for (let index = 0; index < count; index++) {
              const button = buttons.nth(index)
              await button.hover()
              // Settled, not equal to what is wanted: a wrong hover colour must reach the
              // comparison below instead of timing out here.
              await button.evaluate((element) =>
                Promise.all(
                  element.getAnimations().map((animation) => animation.finished),
                ),
              )
              const result = await button.evaluate((element) => {
                const probe = document.createElement("div")
                probe.style.background = "var(--desktop-hover-strong)"
                element.parentElement.append(probe)
                const want = getComputedStyle(probe).backgroundColor
                probe.remove()
                return {
                  label: element.getAttribute("aria-label"),
                  got: getComputedStyle(element).backgroundColor,
                  want,
                }
              })
              seen.push(result.label)
              if (result.got !== result.want)
                failures.push(
                  `${result.label}: hover fills ${result.got}, not ${result.want}`,
                )
            }
            return { failures, measured: { buttons: seen } }
          } finally {
            await close().catch(() => {})
          }
        })

        await attempt(rep, { name: "focus", engine, layout }, async () => {
          const { page, close } = await openPage(browser, { url, layout })
          try {
            await need(page, ".desktop-icon-button:visible", "an icon button")
            const buttons = page.locator(
              ".desktop-icon-button:visible:not(:disabled):not([aria-hidden=true])",
            )
            const count = await buttons.count()
            const failures = []
            const seen = []
            for (let index = 0; index < count; index++) {
              // A key press first, so the focus is the keyboard's (`:focus-visible`);
              // WebKit's Tab skips a button with no tabindex, so it is focused by name.
              await page.keyboard.press("Shift")
              await buttons.nth(index).focus()
              const outline = await page.evaluate(() => {
                const element = document.activeElement
                if (!element?.matches(".desktop-icon-button")) return null
                const style = getComputedStyle(element)
                return {
                  label: element.getAttribute("aria-label"),
                  style: style.outlineStyle,
                  width: style.outlineWidth,
                }
              })
              if (outline === null) failures.push(`button ${index} could not be focused`)
              else {
                seen.push(outline.label)
                if (outline.style === "none" || parseFloat(outline.width) === 0)
                  failures.push(`${outline.label}: focused, draws no outline`)
              }
            }
            return { failures, measured: { buttons: seen } }
          } finally {
            await close().catch(() => {})
          }
        })
      }
    })
  },
)
