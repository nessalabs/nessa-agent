/**
 * Driving and reading the workspace: pane rects and order, opening panes
 * beside through the quick switcher (the same in both layouts), the side
 * columns, and the drop zone the drag announces.
 */
import { CannotRun } from "./cli.mjs"
import { css, keys, names, preferenceEvents, storage } from "./selectors.mjs"

/** Every pane's key, rect and focus, in document order. */
export function panes(page) {
  return page.evaluate(
    ([sel, ghost, focused]) => {
      return [...document.querySelectorAll(sel)]
        .filter((e) => !e.closest(ghost))
        .map((e) => {
          const r = e.getBoundingClientRect()
          return {
            key: e.dataset.paneKey,
            focused: e.matches(focused),
            x: r.left,
            y: r.top,
            w: r.width,
            h: r.height,
          }
        })
    },
    [css.pane, css.dragGhost, css.focusedPane],
  )
}

/** Pane keys ordered as they read: top to bottom within left-to-right columns. */
export async function order(page) {
  const list = await panes(page)
  return list
    .slice()
    .sort((a, b) => Math.round(a.x - b.x) || Math.round(a.y - b.y))
    .map((p) => p.key)
}

export const paneCount = async (page) => (await panes(page)).length

/** What fills the content region (see `content` in selectors.mjs) and where the caret is. */
export function state(page) {
  return page.evaluate(
    ([workspace, pane, focusedPane]) => {
      const a = document.activeElement
      return {
        content: document.querySelector(workspace)?.dataset.content ?? null,
        active: a
          ? `${a.tagName}${a.getAttribute("aria-label") ? `[${a.getAttribute("aria-label")}]` : ""}`
          : null,
        activeInPane: a?.closest(pane)?.dataset.paneKey ?? null,
        activeIsComposer: a?.tagName === "TEXTAREA" && !!a.closest(pane),
        activeOverviewItem:
          a?.closest("[data-overview-item]")?.dataset.overviewItem ?? null,
        focusedPane: document.querySelector(focusedPane)?.dataset.paneKey ?? null,
      }
    },
    [css.workspace, css.pane, css.focusedPane],
  )
}

/**
 * Opens panes beside until there are `n`, picking sessions in the quick
 * switcher (⌘\\, then the i-th row). Fails as could-not-run if the count
 * does not grow — the UI is not what the script expects.
 */
export async function openPanes(page, n) {
  for (let i = 1; (await paneCount(page)) < n; i++) {
    const before = await paneCount(page)
    if (i > n + 3) throw new CannotRun(`could not open ${n} panes (stuck at ${before})`)
    await page.keyboard.press(keys.openBeside)
    await page.waitForTimeout(300)
    for (let j = 0; j <= i; j++) await page.keyboard.press("ArrowDown")
    await page.keyboard.press("Enter")
    await page.waitForTimeout(700)
  }
}

/** Puts the caret in the focused pane's composer, where the window's keys are pressed from. */
export async function focusComposer(page) {
  const composer = page.locator(`${css.focusedPane} textarea`).first()
  if (!(await composer.count()))
    throw new CannotRun(`no composer in the focused pane (${css.focusedPane} textarea)`)
  await composer.click()
  await page.waitForTimeout(200)
}

/** Leaves Settings the way a person does, by its Back button. */
export async function leaveSettings(page) {
  const back = page.getByRole("button", { name: names.leaveSettings }).first()
  if (!(await back.count()))
    throw new CannotRun(`no "${names.leaveSettings}" button in Settings`)
  await back.click()
}

/** Hides the side columns so the panes have the room (⌘B, and ⌥⌘S in three columns). */
export async function hideColumns(page, layout) {
  await page.keyboard.press(keys.toggleSidebar)
  if (layout === "columns") await page.keyboard.press(keys.toggleSessionList)
  await page.waitForTimeout(600)
}

/** Changes the layout the way Settings does: the stored preference and its event. */
export function switchLayout(page, layout) {
  return page.evaluate(
    ([key, event, value]) => {
      localStorage.setItem(key, value)
      window.dispatchEvent(new CustomEvent(event, { detail: value }))
    },
    [storage.layout, preferenceEvents.layout, layout],
  )
}

/** Starts recording the drop zones the drag announces; `take()` returns and clears them. */
export async function recordZones(page) {
  const found = await page.evaluate((sel) => {
    const status = document.querySelector(sel)
    if (!status) return false
    window.__verifyZones = []
    new MutationObserver(() => window.__verifyZones.push(status.textContent)).observe(
      status,
      {
        childList: true,
        characterData: true,
        subtree: true,
      },
    )
    return true
  }, css.dropAnnouncer)
  if (!found) throw new CannotRun(`no drop announcer (${css.dropAnnouncer})`)
  return {
    take: () => page.evaluate(() => window.__verifyZones.splice(0).filter(Boolean)),
    clear: () => page.evaluate(() => (window.__verifyZones = [])),
  }
}

/** Presses a pane's header to lift it, leaving the pointer down past the drag threshold. */
export async function lift(page, index = 0) {
  const header = page.locator(css.paneDragHandle).nth(index)
  const box = await header.boundingBox()
  if (!box) throw new CannotRun(`no pane drag handle ${index} (${css.paneDragHandle})`)
  // Grab left of centre, away from the header's own buttons at its far end.
  const x = box.x + box.width * 0.4
  const y = box.y + box.height / 2
  await page.mouse.move(x, y)
  await page.mouse.down()
  for (let i = 1; i <= 6; i++) await page.mouse.move(x + i * 5, y + i * 5)
  await page.waitForTimeout(100)
  if (!(await page.locator(css.dragGhost).count()))
    throw new CannotRun(
      `pressing and moving pane ${index}'s header did not lift a copy (${css.dragGhost})`,
    )
  return { x: x + 30, y: y + 30 }
}
