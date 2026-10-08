#!/usr/bin/env node
/**
 * The window's shared controls (#632): patterns the app used to draw several
 * ways, each now drawn by one component of the kit (`@nessa-ui/react`) and
 * given the window's inks in one rule. Every instance on the page is held to
 * that component's measured contract.
 */
import { openPage, need, withEngines } from "./lib/browser.mjs"
import { attempt } from "./lib/cli.mjs"
import { main } from "./lib/run.mjs"
import { css, keys } from "./lib/selectors.mjs"

/**
 * Measures every key cap on the page against the kit's `Kbd` as the window
 * skins it (`chrome.css`): 18px tall, at least as wide, 11px medium type, the
 * window's small corner, a 7% fill and the muted ink. In the page.
 */
function measureKeyCaps(selector) {
  const probe = (property, value) => {
    const element = document.createElement("div")
    element.style.setProperty(property, value)
    document.querySelector("[data-surface]").append(element)
    const style = getComputedStyle(element)
    const read = {
      background: style.backgroundColor,
      color: style.color,
      "border-radius": style.borderTopLeftRadius,
    }[property]
    element.remove()
    return read
  }
  const want = {
    fill: probe("background", "color-mix(in oklab, var(--foreground) 7%, transparent)"),
    ink: probe("color", "var(--desktop-muted)"),
    radius: probe("border-radius", "var(--desktop-radius-xs)"),
  }
  const failures = []
  const caps = [...document.querySelectorAll(selector)].filter((cap) => {
    const box = cap.getBoundingClientRect()
    return (
      box.width > 0 && box.height > 0 && getComputedStyle(cap).visibility !== "hidden"
    )
  })
  const seen = []
  for (const cap of caps) {
    const box = cap.getBoundingClientRect()
    const style = getComputedStyle(cap)
    const name = cap.textContent.trim()
    seen.push({ key: name, width: +box.width.toFixed(2), height: box.height })
    if (cap.dataset.slot !== "kbd") {
      failures.push(`${name}: a <kbd> that is not the kit's Kbd`)
      continue
    }
    if (Math.abs(box.height - 18) > 0.5)
      failures.push(`${name}: ${box.height}px tall, not 18`)
    if (box.width < 17.5) failures.push(`${name}: ${box.width}px wide, under 18`)
    if (style.fontSize !== "11px" || style.fontWeight !== "500")
      failures.push(`${name}: type ${style.fontSize} ${style.fontWeight}, not 11px 500`)
    if (style.backgroundColor !== want.fill)
      failures.push(`${name}: fill ${style.backgroundColor}, not ${want.fill}`)
    if (style.color !== want.ink)
      failures.push(`${name}: ink ${style.color}, not ${want.ink}`)
    if (style.borderTopLeftRadius !== want.radius)
      failures.push(`${name}: corner ${style.borderTopLeftRadius}, not ${want.radius}`)
  }
  return { failures, caps: seen }
}

await main(
  {
    name: "shared-controls",
    summary: "key caps: one kit component, measured",
    defaults: { engine: "chromium,webkit" },
    help: `
Checks, per engine and layout:
  keys       every <kbd> in the window (the sidebar's search, the session list's
             search, the quick switcher's rows) is the kit's Kbd: 18px tall and at
             least as wide, 11px medium type, --desktop-radius-xs, a 7% fill and
             --desktop-muted ink. Measured on load and with the switcher open.

Settings is left out (#632 › Settings is redesigned on its own branch).`,
  },
  async ({ options, rep, url }) => {
    await withEngines(options, rep, async (engine, browser) => {
      for (const layout of options.layouts) {
        await attempt(rep, { name: "keys", engine, layout }, async () => {
          const { page, close } = await openPage(browser, { url, layout })
          try {
            await need(page, css.anyReady, "the window")
            await need(page, `${css.keyCap}:visible`, "a key cap")
            const onLoad = await page.evaluate(measureKeyCaps, css.keyCap)
            await page.keyboard.press(keys.switcher)
            await need(
              page,
              `${css.switcherResults} ${css.keyCap}`,
              "the switcher's keys",
            )
            // Measured once the switcher has settled, not while it scales in.
            // (A running session's glyph spins forever, so only the switcher's own count.)
            await page.evaluate((selector) => {
              const switcher = document
                .querySelector(selector)
                ?.closest('[role="dialog"]')
              return Promise.all(
                (switcher?.getAnimations({ subtree: true }) ?? [])
                  .filter(
                    (animation) => animation.effect?.getTiming().iterations !== Infinity,
                  )
                  .map((animation) => animation.finished),
              )
            }, css.switcherResults)
            const inSwitcher = await page.evaluate(
              measureKeyCaps,
              `${css.switcherResults} ${css.keyCap}`,
            )
            const failures = [...onLoad.failures, ...inSwitcher.failures]
            if (onLoad.caps.length === 0) failures.push("no key cap on load")
            return {
              failures,
              measured: { onLoad: onLoad.caps, inSwitcher: inSwitcher.caps },
            }
          } finally {
            await close().catch(() => {})
          }
        })
      }
    })
  },
)
