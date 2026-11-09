/** Target-specific pane observations under the existing settlement budget. */
import { errors } from "playwright"
import { CannotRun } from "./cli.mjs"
import { css } from "./selectors.mjs"

/** Describe the command's destination before its old DOM can look settled. */
export function expectedPaneFocus(before, move = "same") {
  const keys = before.keys
  if (!keys.includes(before.focusedPane))
    throw new CannotRun("the command's focused pane is not mounted")
  if (move === "new") return { keys, added: true }
  if (move === "close") return { keys: keys.filter((key) => key !== before.focusedPane) }
  let index = keys.indexOf(before.focusedPane)
  if (typeof move === "number") index = move
  else if (move === "next") index = Math.min(index + 1, keys.length - 1)
  else if (move === "previous") index = Math.max(index - 1, 0)
  const target = keys[index]
  if (!target) throw new CannotRun("the command's focused pane is not mounted")
  return { keys, target }
}

/** Runs in the page; each sample reads membership, mark and caret together. */
export function observePaneFocus({ selectors, expected }) {
  const started = performance.now()
  const trace = []
  const sample = () => {
    const panes = [...document.querySelectorAll(selectors.pane)].filter(
      (pane) => !pane.closest(selectors.dragGhost),
    )
    const marked = panes.filter((pane) => pane.matches(selectors.focusedPane))
    const active = document.activeElement
    return {
      at: performance.now() - started,
      keys: panes.map((pane) => pane.dataset.paneKey),
      focusedPane: marked.length === 1 ? marked[0].dataset.paneKey : null,
      activeInPane: active?.closest(selectors.pane)?.dataset.paneKey ?? null,
      activeIsComposer:
        active?.tagName === "TEXTAREA" && !!active.closest(selectors.pane),
      blocked: active?.closest(selectors.focusBlocked) != null,
      moving: document
        .getAnimations()
        .some(
          (animation) =>
            ["running", "pending"].includes(animation.playState) &&
            animation.effect?.getTiming?.().iterations !== Infinity,
        ),
    }
  }
  return {
    ready() {
      const state = sample()
      trace.push(state)
      const added = state.keys.filter((key) => !expected.keys.includes(key))
      const membership = expected.added
        ? added.length === 1 &&
          state.keys.length === expected.keys.length + 1 &&
          expected.keys.every((key) => state.keys.includes(key))
        : state.keys.length === expected.keys.length &&
          expected.keys.every((key) => state.keys.includes(key))
      const target = expected.added ? added[0] : (expected.target ?? state.focusedPane)
      return (
        membership &&
        target != null &&
        state.focusedPane === target &&
        state.activeInPane === target &&
        state.activeIsComposer &&
        !state.blocked &&
        !state.moving
      )
    },
    report: () => ({ trace, after: sample() }),
  }
}

/** One budget covers finite motion and the expected pane's actual caret. */
export async function paneFocusSettled(page, expected, timeout = 3000) {
  const owner = await page.evaluateHandle(observePaneFocus, { selectors: css, expected })
  let converged = false
  try {
    try {
      const ready = await page.waitForFunction((held) => held.ready(), owner, {
        polling: "raf",
        timeout,
      })
      await ready.dispose()
      converged = true
    } catch (error) {
      if (!(error instanceof errors.TimeoutError)) throw error
    }
    return { converged, ...(await owner.evaluate((held) => held.report())) }
  } finally {
    await owner.dispose()
  }
}

/** Several effect owners may queue movement frames; every one is checked later. */
export function requirePendingMovement(captured) {
  if (!Number.isInteger(captured) || captured < 1)
    throw new CannotRun(`pane movement queued ${captured} frames; none can be retired`)
}
