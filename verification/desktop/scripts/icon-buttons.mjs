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
  hover      the pointer over one fills it with the window's stronger hover
  focus      keyboard focus on one draws an outline`,
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
            const button = page.locator(".desktop-icon-button:visible").first()
            await button.hover()
            await page.waitForTimeout(300)
            const result = await button.evaluate((element) => {
              const probe = document.createElement("div")
              probe.style.background = "var(--desktop-hover-strong)"
              element.parentElement.append(probe)
              const want = getComputedStyle(probe).backgroundColor
              probe.remove()
              return { got: getComputedStyle(element).backgroundColor, want }
            })
            const failures =
              result.got === result.want
                ? []
                : [`hover fills ${result.got}, not ${result.want}`]
            return { failures, measured: result }
          } finally {
            await close().catch(() => {})
          }
        })

        await attempt(rep, { name: "focus", engine, layout }, async () => {
          const { page, close } = await openPage(browser, { url, layout })
          try {
            await need(page, ".desktop-icon-button:visible", "an icon button")
            // A key press first, so the focus is the keyboard's (`:focus-visible`);
            // WebKit's Tab skips a button with no tabindex, so it is focused by name.
            await page.keyboard.press("Shift")
            await page.locator(".desktop-icon-button:visible").first().focus()
            const outline = await page.evaluate(() => {
              const element = document.activeElement
              if (!element?.matches(".desktop-icon-button")) return null
              const style = getComputedStyle(element)
              return {
                label: element.getAttribute("aria-label"),
                style: style.outlineStyle,
                width: style.outlineWidth,
                focusVisible: element.matches(":focus-visible"),
              }
            })
            const failures =
              outline === null
                ? ["an icon button could not be focused"]
                : outline.style === "none" || parseFloat(outline.width) === 0
                  ? [`focused icon button draws no outline (${JSON.stringify(outline)})`]
                  : []
            return { failures, measured: outline }
          } finally {
            await close().catch(() => {})
          }
        })
      }
    })
  },
)
