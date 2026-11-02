import { css, keys } from "./selectors.mjs"
import { settled } from "./workspace.mjs"

/** A mounted copy in a folded column is not a row a person can act on. */
export function visibleSessionRow(title) {
  const row = [...document.querySelectorAll("[data-session-row]")].find((item) => {
    if (item.querySelector(".desktop-list-row-label")?.textContent !== title) return false
    const box = item.getBoundingClientRect()
    return (
      item.checkVisibility({ visibilityProperty: true }) &&
      box.width > 0 &&
      box.height > 0 &&
      box.right > 0 &&
      box.left < innerWidth &&
      box.bottom > 0 &&
      box.top < innerHeight
    )
  })
  return row
    ? { id: row.dataset.sessionRow, list: Boolean(row.closest(".workspace-list")) }
    : null
}

/** Reacquire after layout changes; reveal a folded sidebar through its normal chord. */
export async function sessionRowForRemoval(page, title) {
  let target = await page.evaluate(visibleSessionRow, title)
  if (
    !target &&
    (await page.locator(css.workspace).getAttribute("data-sidebar")) === "closed"
  ) {
    await page.keyboard.press(keys.toggleSidebar)
    await settled(page)
    target = await page.evaluate(visibleSessionRow, title)
  }
  if (!target) throw new Error(`no visible session row titled ${title}`)
  return target.list
    ? page.locator(`.workspace-list [data-session-row="${target.id}"]`)
    : page.locator(`.workspace-sidebar [data-session-row="${target.id}"]`).first()
}
