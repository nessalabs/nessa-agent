/**
 * MCP Apps in the window: an app's documents are cross-origin to the page,
 * so they are read through Playwright's frames, never through the page. The
 * window's approval card for an app's call is the page's own, and is read
 * through the page.
 */
import { CannotRun } from "./cli.mjs"
import { css, names, selectorFor } from "./selectors.mjs"
import { settled } from "./workspace.mjs"

/**
 * The app's own document in the frame drawn in `place` (inline, pane,
 * window): the sandbox proxy's one child. Returns the host's frame element,
 * the proxy's frame, and the app's.
 */
export async function appFrame(page, place, timeout = 10_000) {
  const until = Date.now() + timeout
  while (Date.now() < until) {
    const element = await page.$(selectorFor.appFrameIn(place))
    const proxy = await element?.contentFrame()
    const app = proxy?.childFrames()[0]
    if (app && !app.isDetached()) return { element, proxy, app }
    await page.waitForTimeout(100)
  }
  throw new CannotRun(`no app document in the ${place} frame`)
}

/**
 * Wait for current finite host motion before acting in the discovered app.
 * Frame-local actionability cannot see a moving ancestor (#906's `moving-frame`
 * browser check). This does not initialize the app or cover later motion or
 * navigation. A frame that detaches during the bounded wait is refused;
 * settlement never looks up a replacement. The handles are captured before
 * waiting, so a caller rewriting its carrier cannot change the target.
 */
export async function settleAppFrame(page, frames, timeout = 3000) {
  if (!Number.isFinite(timeout) || timeout <= 0)
    throw new CannotRun("app host settlement needs a positive finite timeout")
  const { element, proxy, app } = frames
  await settled(page, timeout)
  if (proxy.isDetached() || app.isDetached())
    throw new CannotRun("the app frame detached while its host settled")
  return { element, proxy, app }
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
 * What is wrong with `count` transcript steps drawn for one call, or null
 * when there is exactly one. A step is drawn per tool part of a turn, so a
 * second one is the same call drawn again (#418).
 */
export function oneCard(count) {
  if (count === 1) return null
  if (count === 0) return "the call has no step in the transcript"
  return `the call is drawn as ${count} transcript steps, not one (#418)`
}

/** `text` and nothing else, as a `hasText` pattern: Playwright's string matches a part. */
export const exactly = (text) =>
  new RegExp(`^${text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`)

/**
 * The locator for the first visible review card an app asked for, whose head
 * is exactly `names.appAsks(server, tool)`. A tool's name may hold any
 * characters (the protocol bounds only its bytes), so no part of a head stands
 * for a name: the whole head does. One rule for the card's appearing and its
 * going (`approvalShown`, `approvalGone`): a hidden card does not stand for a
 * visible one.
 */
export const approvalCardFor = (page, server, tool) =>
  page
    .locator(css.appApprovalCard)
    .filter({
      has: page.locator(css.approvalHeadWords, {
        hasText: exactly(names.appAsks(server, tool)),
      }),
      visible: true,
    })
    .first()

/** Waits up to `ms` for `approvalCardFor`'s card to show: the card, or `null`. */
export async function approvalShown(page, server, tool, ms = 10_000) {
  const card = approvalCardFor(page, server, tool)
  try {
    await card.waitFor({ state: "visible", timeout: ms })
    return card
  } catch {
    return null
  }
}

/**
 * Waits up to `ms` until `approvalCardFor` matches no card; true when it
 * matches none by then, false when it still matches one at the bound. A card
 * shown when the wait starts is waited out, not sampled. Errors other than the
 * timeout (a closed page) propagate.
 */
export async function approvalGone(page, server, tool, ms) {
  try {
    await approvalCardFor(page, server, tool).waitFor({ state: "hidden", timeout: ms })
    return true
  } catch (error) {
    if (error?.name === "TimeoutError") return false
    throw error
  }
}
