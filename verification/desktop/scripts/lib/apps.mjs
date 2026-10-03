/**
 * MCP Apps in the window: an app's documents are cross-origin to the page,
 * so they are read through Playwright's frames, never through the page.
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
