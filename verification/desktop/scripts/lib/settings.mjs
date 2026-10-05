/**
 * Settings › Connections › Integrations, as the checks reach and measure it:
 * `responsive.mjs` (`integrations-narrow`, on whatever the page shows) and
 * `mcp-servers-gateway.mjs` (`narrow`, with servers listed, the form and the
 * inspection open, over a real gateway) hold the same fit contract.
 */
import { need } from "./browser.mjs"
import { CannotRun } from "./cli.mjs"
import { css, keys, names } from "./selectors.mjs"
import { frames, settled } from "./workspace.mjs"

/** Opens Settings on Connections › Integrations, and waits for its card. */
export async function openIntegrations(page) {
  if (!(await page.$(css.settings))) await page.keyboard.press(keys.settings)
  await need(page, css.settings, "Settings")
  await page.locator(css.settingsCategory, { hasText: names.connections }).click()
  await page.locator(css.settingsTab, { hasText: names.integrations }).click()
  await need(page, css.mcpGroup, "Integrations' MCP servers")
  await settled(page)
}

/**
 * At each width, whether Integrations fits (U29, U30): Settings does not
 * scroll sideways, nor reach past the viewport (the window under it, whose
 * own minimum width is not Settings', is reported as `windowScroll`); every part of the tab — rows, buttons, fields, code,
 * badges — lies inside its card (half a pixel's give); nothing is clipped,
 * a field's value included; the sidebar is folded
 * when the page would be under its 420px (the existing fold); and where the
 * page is that narrow, a server row's actions sit under its text rather
 * than beside it. Returns what was measured and what broke.
 */
export async function integrationsFit(page, widths) {
  const seen = []
  const failures = []
  for (const width of widths) {
    await page.setViewportSize({ width, height: 800 })
    await frames(page, 2)
    await settled(page)
    const measured = await page.evaluate(
      (sel) => {
        const settings = document.querySelector(sel.settings)
        const panel = document.querySelector(sel.panel)
        const cards = panel?.querySelectorAll(`${sel.card}, ${sel.groupHeading}`) ?? []
        const scrollers = settings?.querySelectorAll(sel.scrollers) ?? []
        // A selector that finds nothing measures nothing: say so, not "fits".
        // Each part of a list on its own, so one renamed class is not hidden
        // by the others still matching.
        const none = (scope, list) =>
          list
            .split(",")
            .map((each) => each.trim())
            .filter((each) => !scope?.querySelector(each))
        const missing = [
          !settings && sel.settings,
          !panel && sel.panel,
          ...none(panel, `${sel.card}, ${sel.groupHeading}`),
          ...none(settings, sel.scrollers),
        ].filter(Boolean)
        if (missing.length > 0) return { missing }
        const sidebar =
          parseFloat(
            getComputedStyle(settings).getPropertyValue("--settings-sidebar-w"),
          ) || 0
        const page = Math.round(
          settings.getBoundingClientRect().width -
            (settings.dataset.sidebar === "open" ? sidebar : 0),
        )
        // Settings' own surface, and each scroller in it: the window under it
        // has a minimum width of its own (480px), which is not Settings' to fit.
        const box = settings.getBoundingClientRect()
        const sideways = Math.max(
          Math.round(box.right - innerWidth),
          ...[settings, ...scrollers].map((each) => each.scrollWidth - each.clientWidth),
        )
        const scroller = document.scrollingElement
        const outside = []
        for (const card of cards) {
          const box = card.getBoundingClientRect()
          for (const part of card.querySelectorAll("*")) {
            const r = part.getBoundingClientRect()
            if (r.width === 0 && r.height === 0) continue
            if (r.left < box.left - 0.5 || r.right > box.right + 0.5)
              outside.push(
                `${part.tagName.toLowerCase()}${part.className ? `.${String(part.className).split(" ")[0]}` : ""} ` +
                  `${Math.round(r.left)}–${Math.round(r.right)} outside ${Math.round(box.left)}–${Math.round(box.right)}`,
              )
          }
        }
        // Clipped: what a box holds is wider than it shows — a scroller's
        // content, or a field's value (an input never wraps, so a value
        // wider than its field is cut off where it is read).
        const field = (each) => each.tagName === "TEXTAREA" || each.tagName === "INPUT"
        const overflowing = [...panel.querySelectorAll("*")]
          .filter((each) => each.scrollWidth > each.clientWidth + 1)
          .filter((each) => field(each) || getComputedStyle(each).overflowX !== "visible")
          .map(
            (each) =>
              `${each.tagName.toLowerCase()}.${String(each.className).split(" ")[0]}` +
              (field(each)
                ? `[${each.getAttribute("aria-label") ?? each.id}] ${each.clientWidth}<${each.scrollWidth}`
                : ""),
          )
        const rows = [...document.querySelectorAll(sel.row)].map((row) => {
          const text = row.querySelector(sel.text)?.getBoundingClientRect()
          const actions = row.querySelector(sel.actions)?.getBoundingClientRect()
          return {
            name: row.getAttribute("data-mcp-server"),
            stacked: Boolean(text && actions && actions.top >= text.bottom - 1),
          }
        })
        return {
          sidebar: settings.dataset.sidebar,
          page,
          sideways,
          windowScroll: scroller.scrollWidth - scroller.clientWidth,
          outside: outside.slice(0, 6),
          outsideCount: outside.length,
          overflowing: overflowing.slice(0, 6),
          rows,
        }
      },
      {
        settings: css.settings,
        panel: css.settingsPanel,
        row: css.mcpRow,
        text: css.mcpRowText,
        actions: css.mcpRowActions,
        card: css.settingsCard,
        groupHeading: css.settingsGroupHeading,
        scrollers: css.settingsScrollers,
      },
    )
    if (measured.missing)
      throw new CannotRun(
        `${width}px: found nothing to measure (${measured.missing.join("; ")}). The UI may be mid-change; update lib/selectors.mjs if it moved.`,
      )
    seen.push({ width, ...measured })
    if (measured.sideways > 0)
      failures.push(`${width}px: Settings scrolls ${measured.sideways}px sideways`)
    if (measured.outsideCount > 0)
      failures.push(
        `${width}px: ${measured.outsideCount} parts outside their card: ${measured.outside.join("; ")}`,
      )
    if (measured.overflowing.length > 0)
      failures.push(`${width}px: clipped: ${measured.overflowing.join(", ")}`)
    if (measured.sidebar === "open" && measured.page < 420)
      failures.push(`${width}px: sidebar drawn with a ${measured.page}px page (< 420)`)
    if (measured.page < 420)
      for (const row of measured.rows.filter((each) => !each.stacked))
        failures.push(
          `${width}px: ${row.name}'s actions are beside its text in a ${measured.page}px page`,
        )
  }
  return { seen, failures }
}
