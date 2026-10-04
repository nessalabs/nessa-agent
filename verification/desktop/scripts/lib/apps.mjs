/**
 * MCP Apps in the window: an app's documents are cross-origin to the page,
 * so they are read through Playwright's frames, never through the page. The
 * window's approval card for an app's call is the page's own, and is read
 * through the page.
 */
import { CannotRun } from "./cli.mjs"
import { css } from "./selectors.mjs"

/**
 * The app's own document in the frame drawn in `place` (inline, pane,
 * window): the sandbox proxy's one child. Returns the host's frame element,
 * the proxy's frame, and the app's.
 */
export async function appFrame(page, place, timeout = 10_000) {
  const until = Date.now() + timeout
  while (Date.now() < until) {
    const element = await page.$(css.appFrameIn(place))
    const proxy = await element?.contentFrame()
    const app = proxy?.childFrames()[0]
    if (app && !app.isDetached()) return { element, proxy, app }
    await page.waitForTimeout(100)
  }
  throw new CannotRun(`no app document in the ${place} frame`)
}

/**
 * What is wrong with `count` inline frames drawn for one call, or null when
 * there is exactly one. Each frame is a mount: a second one repeats the app's
 * calls and the reviews they open (#418).
 */
export function oneMount(count) {
  if (count === 1) return null
  if (count === 0) return "the call has no inline app frame"
  return `the call is drawn as ${count} inline app frames, not one (#418)`
}

/**
 * `tool` as a whole name, case and all: `app_delete_row` is not named by
 * `app_delete_rows` or `APP_DELETE_ROW`, as a plain `hasText` would take it.
 */
export const namesTool = (tool) =>
  new RegExp(`(?<!\\w)${tool.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?!\\w)`)

/**
 * The first visible approval card that names `tool` (`namesTool`). One rule
 * for the card's appearing and its going (`approvalNaming`, `approvalGone`):
 * a hidden card that names the tool does not stand for a visible one.
 */
export const approvalCardNaming = (page, tool) =>
  page
    .locator(css.approvalCard, { hasText: namesTool(tool) })
    .filter({ visible: true })
    .first()

/** The card `approvalCardNaming` matches, once it shows; `null` when none does within `ms`. */
export async function approvalNaming(page, tool, ms = 10_000) {
  const card = approvalCardNaming(page, tool)
  try {
    await card.waitFor({ state: "visible", timeout: ms })
    return card
  } catch {
    return null
  }
}

/**
 * Waits up to `ms` until `approvalCardNaming` matches no card; true when it
 * matches none by then, false when it still matches one at the bound. A card
 * shown when the wait starts is waited out, not sampled. Errors other than the
 * timeout (a closed page) propagate.
 */
export async function approvalGone(page, tool, ms) {
  try {
    await approvalCardNaming(page, tool).waitFor({ state: "hidden", timeout: ms })
    return true
  } catch (error) {
    if (error?.name === "TimeoutError") return false
    throw error
  }
}
